extern crate serde;

use std::borrow::Borrow;
use clap::{Parser};
use std::collections::HashMap;
use serde_json::{Value, json};
use std::cell::RefCell;
use chrono::{Utc, DateTime, Local};
use std::cmp::Reverse;
use std::io::Write;
use futures::future::join_all;
use std::sync::Arc;
use std::sync::Mutex;
use tokio::time::{sleep, Duration};


/// A command line utility for interfacing with lwproxy in scheduling container
/// image assurance scans against JFrog registries.
#[derive(Parser, Clone)]
#[clap(version = "1.0", author = ">")]
struct Opts {
    #[clap(short, long)]
    /// Registry endpoint/s with port must be specified. i.e. test.jfrog.io:443,test2.jfrog.io:443
    registries: String,
    /// Optional - if not specified registries will be enumerated for package type = Docker
    #[clap(short='k', long)]
    repo_keys: Vec<String>,
    #[clap(short, long, default_value = "1")]
    num_images: usize,
    #[clap(short, long, default_value = "1500")]
    scans_hour: usize,
    ///Full URL must be specified such as http://192.168.1.41:9080
    #[clap(short='l', long)]
    proxy_scanner_address: String,


}

#[derive(Debug, Clone)]
pub struct Repository{
    pub name: String,
    pub tags: RefCell<Vec<(String, DateTime<Utc>)>>,
}

#[derive(Debug, Clone)]
pub struct Endpoint{
    pub name: String,
    pub repo_keys: RefCell<HashMap<String, Vec<Repository>>>,
    pub username: String,
    pub password: String
}


async fn get_tags(endpoint: &Endpoint, repo_key: String, image_repository: String)-> RefCell<Vec<(String, DateTime<Utc>)>> {
    let client = reqwest::Client::new();
    let url = format!("https://{}/artifactory/api/docker/{}/v2/{}/tags/list", endpoint.name, repo_key.replace("\"", ""), image_repository.replace("\"", ""));
    let resp = client.get(url).basic_auth(&endpoint.username, Some(&endpoint.password)).send().await.unwrap().json::<serde_json::Value>().await.unwrap();
    let tags: RefCell<Vec<(String, DateTime<Utc>)>> = RefCell::new(Vec::new());



    for tag in resp.as_object().unwrap()["tags"].as_array().unwrap(){
        let url = format!("https://{}/artifactory/api/docker/{}/v2/{}/manifests/{}", endpoint.name, repo_key.replace("\"", ""), image_repository.replace("\"", ""), tag.to_string().replace("\"", ""));
        let resp = client.get(url).basic_auth(&endpoint.username, Some(&endpoint.password)).send().await.unwrap().json::<serde_json::Value>().await.unwrap();

        let history = resp.as_object().unwrap()["history"].as_array().unwrap().get(0).unwrap();
        let v:Value = serde_json::from_str(history.as_object().unwrap().get("v1Compatibility").unwrap().as_str().unwrap()).unwrap();
        let created: DateTime<Utc> = v.as_object().unwrap()["created"].as_str().unwrap().parse().unwrap();

        tags.borrow_mut().push((tag.to_string().replace("\"", ""), created) );
    }

    tags.borrow_mut().sort_by_key(|k| Reverse(k.1));
    return tags
}

async fn get_images(endpoints: &Vec<Endpoint>){

    let client = reqwest::Client::new();


    for endpoint in endpoints {
        let mut repo_keys = endpoint.repo_keys.borrow_mut();


        for (repo_key, _) in repo_keys.clone().iter() {
            let url = format!("https://{}/artifactory/api/docker/{}/v2/_catalog", endpoint.name, repo_key.replace("\"", ""));
            let response = client.get(url).basic_auth(&endpoint.username, Some(&endpoint.password)).send().await.expect(format!("ERROR: connecting to {:?}", &endpoint.name).as_str());
            let resp = response.json::<serde_json::Value>().await.expect(format!("ERROR: connecting to {:?}", &endpoint.name).as_str());
            let repos = resp.as_object().unwrap()["repositories"].as_array().unwrap();

            let mut repositories: Vec<Repository> = Vec::new();
            for repo in repos {
                let tags = get_tags(&endpoint, repo_key.clone(), repo.to_string()).await;
                let repository = Repository{name: repo.to_string().replace("\"", ""), tags: tags};
                repositories.push(repository);
            }
            repo_keys.insert(repo_key.to_string(), repositories);
        }

    }


}

async fn get_repo_keys(endpoints: &Vec<Endpoint>){

    let client = reqwest::Client::new();

    for endpoint in endpoints {
        if endpoint.repo_keys.borrow().len() == 0{

            let url = format!("https://{}/artifactory/api/repositories", endpoint.name);
            let resp = client.get(url).basic_auth(&endpoint.username, Some(&endpoint.password)).send().await.unwrap();
            let repo_keys = resp.json::<serde_json::Value>().await.unwrap();

            println!("REPO_KEYS: {}", repo_keys);
            for repo_key in repo_keys.as_array().unwrap(){
                for (key, value) in repo_key.as_object().unwrap() {
                    if key == "packageType" && value.as_str().unwrap() == "Docker"{
                       endpoint.repo_keys.borrow_mut().insert(repo_key.get("key").unwrap().to_string().replace("\"", ""), Vec::new());
                    }
                }
            }

        }
    }


}



fn set_up_scan_jobs(endpoints: &Vec<Endpoint>, options: &Opts) -> Vec<serde_json::Value>{
    let mut scan_jobs: Vec<serde_json::Value> = Vec::new();

   for endpoint in endpoints.iter(){
       for (name, repos) in endpoint.borrow().repo_keys.borrow().clone().iter(){
           for repo in repos {
               let num_images = if options.num_images < repo.tags.borrow().len() { options.num_images  } else { repo.tags.borrow().len()  };
               //println!("NUM IMAGES: {}:{:?}", num_images, endpoint.repo_keys);
               for tag in repo.tags.borrow()[..num_images].iter(){
                   let registry = format!("{}/artifactory/api/docker/{}", endpoint.name, name);
                   let obj = json!({"registry": registry, "image_name": repo.name, "tag": tag.0});
                   scan_jobs.push(obj);
               }
           }

       }
   }
    return scan_jobs
}



async fn scan_image(scan_job: &serde_json::Value, opts: &Opts, summary: &Arc<Mutex<Vec<serde_json::Value>>>){
    log::debug!("In SCAN IMAGES");
    let client = reqwest::Client::new();
    let url = format!("{}/v1/scan", opts.proxy_scanner_address);
    let registry = &scan_job.as_object().unwrap().get("registry").unwrap().to_string().replace("\"", "");
    let image_name = &scan_job.as_object().unwrap().get("image_name").unwrap().to_string().replace("\"", "");
    let image_tag = &scan_job.as_object().unwrap().get("tag").unwrap().to_string().replace("\"", "");


        match client.post(&url).json(&scan_job).send().await {
            Ok(response) => {
                match response.json::<serde_json::Value>().await{
                    Ok(resp) => {
                        let status_code = resp.as_object().unwrap()["status_code"].as_u64().unwrap();
                        if status_code == 200 as u64 {
                            println!("[SCAN REQUEST] REGISTRY: {:#?} IMAGE: {:#?}:{:#?} [RESULT] SUCCESS -> {}", registry, image_name, image_tag, resp);
                        }else{
                            println!("[SCAN REQUEST] REGISTRY: {:#?} IMAGE: {:#?}:{:#?} [RESULT] SUCCESS -> {}", registry, image_name, image_tag, resp);
                            let obj = json!({"registry": registry, "image_name": image_name, "tag": image_tag, "error": resp});
                            summary.lock().unwrap().push(obj);
                        }

                    },
                    Err(e) => {
                        println!("BIG ERROR 1");
                        println!("[SCAN REQUEST] REGISTRY: {:#?} IMAGE: {:#?}:{:#?} [RESULT] FAILURE -> {}", registry, image_name, image_tag, e);
                        let obj = json!({"registry": registry, "image_name": image_name, "tag": image_tag, "error": e.to_string()});
                        summary.lock().unwrap().push(obj);
                    }
                }
            },
            Err(e) => {
                println!("BIG ERROR 2");
                println!("[SCAN REQUEST] -> REGISTRY: {:#?} IMAGE: {:#?}:{:#?} [RESULT] FAILURE -> {}", registry, image_name, image_tag, e);
                //let obj = json!({"registry": registry, "image_name": image_name, "tag": image_tag, "error": e.to_string()});
                //summary.lock().unwrap().push(obj);
                panic!("{:?}", e);
            }
        };


}

pub fn setup_logging(){

    env_logger::builder().format(|buf, record| {
            writeln!(buf, "{} [{}] - {}", Local::now().format("%Y-%m-%dT%H:%M:%S"),record.level(), record.args())
        })
        .init();
}

fn print_registries(registries: &Vec<&str>){
    let mut temp_registries: Vec<&str> = vec![];

    for registry in registries{
        let chunks: Vec<&str> = registry.split('[').collect();
        temp_registries.push(chunks[0]);
    }
    println!("REGISTRIES: {:?}", temp_registries);

}

fn print_summary(scan_jobs: &Vec<serde_json::Value>, summary: &Arc<Mutex<Vec<serde_json::Value>>>){
    println!("LENGTH OF SCAN JOBS: {}", scan_jobs.len());
    println!("SUCCESSFUL SCAN JOBS: {}", scan_jobs.len() - summary.lock().unwrap().len());
    println!("FAILED SCANS:\n{:?}", summary.lock().unwrap());
}

#[tokio::main]
async fn main() {
    let opts = Opts::parse();
    let options: Opts = opts.clone();
    let summary: Arc<Mutex<Vec<serde_json::Value>>> = Arc::new(Mutex::new(vec![]));
    setup_logging();

    let registries: Vec<&str> = options.registries.split(',').collect();
    print_registries(&registries);

    let endpoints: Vec<Endpoint> = registries.into_iter().map(|x| {

        let registry_info: Vec<&str> = x.split(&['[',']', ':'][..]).collect();
        let repo_keys: RefCell<HashMap<String, Vec<Repository>>> = RefCell::new(HashMap::new());
        for repo_key in opts.repo_keys.clone(){
            repo_keys.borrow_mut().insert(repo_key, Vec::new());
        }

        let endpoint = Endpoint{name: format!("{}:{}", registry_info[0], registry_info[1]), repo_keys: repo_keys, username:registry_info[2].to_string(), password:registry_info[3].to_string() };
        log::info!("ENDPOINT: {:?}", endpoint);
        return endpoint
    }).rev().collect();


    get_repo_keys(&endpoints).await;
    get_images(&endpoints).await;
    let scan_jobs = set_up_scan_jobs(&endpoints, &options);



    let mut scan_count = 0;
    let scans_per_hour = options.scans_hour;
    let done: Arc<Mutex<bool>> = Arc::new(Mutex::new(false));
    let mut scans_per_hour = scans_per_hour;

    println!("TOTAL SCAN JOBS FOUND: {:?}", scan_jobs.len());
    if scan_jobs.len() < scans_per_hour{
        scans_per_hour = scan_jobs.len()
    }

    loop {
        if scans_per_hour > scan_jobs.len(){

            println!("SCANNING IMAGES {} - {} :: {}", scan_count + 1, scan_count + scans_per_hour, Utc::now());
            let futures = scan_jobs.iter().map(|scan_job| scan_image(scan_job, &options, &summary));
            join_all(futures).await;
            *done.lock().unwrap() = true;
            print_summary(&scan_jobs, &summary);
            break;


        }else{
            if scan_count + scans_per_hour >= scan_jobs.len(){
                println!("SCANNING IMAGES {} - {}  :: {}", scan_count + 1, scan_jobs.len(), Utc::now());
                let futures = scan_jobs[scan_count..scan_jobs.len()].iter().map(|scan_job| scan_image(scan_job, &options, &summary));
                join_all(futures).await;
                *done.lock().unwrap() = true;
                print_summary(&scan_jobs, &summary);
                break;


            }else{
                println!("SCANNING IMAGES {} - {} :: {}", scan_count + 1, scan_count + scans_per_hour, Utc::now());
                let futures = scan_jobs[scan_count..scan_count + scans_per_hour].iter().map(|scan_job| scan_image(scan_job, &options, &summary));
                join_all(futures).await;
                scan_count = scan_count + scans_per_hour;
            }
        }

        if *done.lock().unwrap(){
            break;
        }

       sleep(Duration::from_millis(3600000)).await;
    }



}