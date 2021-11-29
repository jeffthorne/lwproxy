extern crate serde;

use std::borrow::Borrow;
use clap::{AppSettings, Parser};
use std::collections::HashMap;
use serde::{Deserialize, Serialize};
use serde_json::{Result, Value, json};
use std::cell::RefCell;
use chrono::{Utc, DateTime, Local};
use std::cmp::Reverse;
use std::env;
use std::io::Write;


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


fn get_tags(endpoint: &Endpoint, repo_key: String, image_repository: String, opts: &Opts)-> RefCell<Vec<(String, DateTime<Utc>)>> {
    let client = reqwest::blocking::Client::new();
    let url = format!("https://{}/artifactory/api/docker/{}/v2/{}/tags/list", endpoint.name, repo_key.replace("\"", ""), image_repository.replace("\"", ""));
    let resp = client.get(url).basic_auth(&endpoint.username, Some(&endpoint.password)).send().unwrap().json::<serde_json::Value>().unwrap();
    let mut tags: RefCell<Vec<(String, DateTime<Utc>)>> = RefCell::new(Vec::new());



    for tag in resp.as_object().unwrap()["tags"].as_array().unwrap(){
        let url = format!("https://{}/artifactory/api/docker/{}/v2/{}/manifests/{}", endpoint.name, repo_key.replace("\"", ""), image_repository.replace("\"", ""), tag.to_string().replace("\"", ""));
        let resp = client.get(url).basic_auth(&endpoint.username, Some(&endpoint.password)).send().unwrap().json::<serde_json::Value>().unwrap();

        let history = resp.as_object().unwrap()["history"].as_array().unwrap().get(0).unwrap();
        let v:Value = serde_json::from_str(history.as_object().unwrap().get("v1Compatibility").unwrap().as_str().unwrap()).unwrap();
        let created: DateTime<Utc> = v.as_object().unwrap()["created"].as_str().unwrap().parse().unwrap();

        tags.borrow_mut().push((tag.to_string().replace("\"", ""), created) );
    }

    tags.borrow_mut().sort_by_key(|k| Reverse(k.1));
    return tags
}

fn get_images(endpoints: &Vec<Endpoint>, opts: &Opts){

    let client = reqwest::blocking::Client::new();


    for endpoint in endpoints {
        let mut repo_keys = endpoint.repo_keys.borrow_mut();

        for (repo_key, _) in repo_keys.clone().iter() {
            let url = format!("https://{}/artifactory/api/docker/{}/v2/_catalog", endpoint.name, repo_key.replace("\"", ""));
            let resp = client.get(url).basic_auth(&endpoint.username, Some(&endpoint.password)).send().unwrap().json::<serde_json::Value>().unwrap();
            println!("RESP: {}", resp);
            let repos = resp.as_object().unwrap()["repositories"].as_array().unwrap();

            let mut repositories: Vec<Repository> = Vec::new();
            for repo in repos {
                let tags = get_tags(&endpoint, repo_key.clone(), repo.to_string(), opts);
                let repository = Repository{name: repo.to_string().replace("\"", ""), tags: tags};
                repositories.push(repository);
            }
            repo_keys.insert(repo_key.to_string(), repositories);
        }

    }


}

fn get_repo_keys(endpoints: &Vec<Endpoint>, opts: &Opts){

    let client = reqwest::blocking::Client::new();

    for endpoint in endpoints {
        if endpoint.repo_keys.borrow().len() == 0{

            let url = format!("https://{}/artifactory/api/repositories", endpoint.name);
            let repo_keys = client.get(url).basic_auth(&endpoint.username, Some(&endpoint.password)).send().unwrap().json::<serde_json::Value>().unwrap();


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
               let num_tags = if options.num_images < repo.tags.borrow().len() { options.num_images  } else { repo.tags.borrow().len()  };
               for tag in repo.tags.borrow()[..num_tags].iter(){
                   let registry = format!("{}/artifactory/api/docker/{}", endpoint.name, name);
                   let obj = json!({"registry": registry, "image_name": repo.name, "tag": tag.0});
                   scan_jobs.push(obj);
               }
           }

       }
   }
    return scan_jobs
}

fn scan_images(scan_jobs: Vec<serde_json::Value>, opts: &Opts){
    let client = reqwest::blocking::Client::new();
    let url = format!("{}/v1/scan", opts.proxy_scanner_address);

    for scan_job in scan_jobs.iter(){
        log::info!("SCHEDULING SCAN FOR -> REGISTRY: {:#?} IMAGE: {:#?}:{:#?}",
            &scan_job.as_object().unwrap().get("registry").unwrap().to_string().replace("\"", ""),
            &scan_job.as_object().unwrap().get("image_name").unwrap().to_string().replace("\"", ""),
            &scan_job.as_object().unwrap().get("tag").unwrap().to_string().replace("\"", "")
        );

        let resp = match client.post(&url).json(&scan_job).send() {
            Ok(response) => {
                log::info!("SCAN RESULT FOR -> REGISTRY: {:#?} IMAGE: {:#?}:{:#?}. Status: {:?}. Result: {:#?}",
                &scan_job.as_object().unwrap().get("registry").unwrap().to_string().replace("\"", ""),
                &scan_job.as_object().unwrap().get("image_name").unwrap().to_string().replace("\"", ""),
                &scan_job.as_object().unwrap().get("tag").unwrap().to_string().replace("\"", ""),
                response.status(),
                response.json::<serde_json::Value>().unwrap().as_object().unwrap().get("ok").unwrap().to_string()
                );
            },
            Err(e) => println!("ERROR: {:?}", e)
        };

    }
}

pub fn setup_logging(){

    env_logger::builder()
        .format(|buf, record| {
            writeln!(buf,
                     "{} [{}] - {}",
                     Local::now().format("%Y-%m-%dT%H:%M:%S"),
                     record.level(),
                     record.args()
            )
        })
        .init();
}

fn main() {
    let opts = Opts::parse();
    let options: Opts = opts.clone();
    setup_logging();

    let registries: Vec<&str> = options.registries.split(',').collect();
    println!("REGISTRIES: {:?}", registries);

    let mut endpoints: Vec<Endpoint> = registries.into_iter().map(|x| {

        let registry_info: Vec<&str> = x.split(&['[',']', ':'][..]).collect();
        let mut repo_keys: RefCell<HashMap<String, Vec<Repository>>> = RefCell::new(HashMap::new());
        for repo_key in opts.repo_keys.clone(){
            repo_keys.borrow_mut().insert(repo_key, Vec::new());
        }

        let mut endpoint = Endpoint{name: format!("{}:{}", registry_info[0], registry_info[1]), repo_keys: repo_keys, username:registry_info[2].to_string(),
                                    password:registry_info[3].to_string() };
        println!("ENDPOINT: {:?}", endpoint);
        return endpoint
    }).rev().collect();

    get_repo_keys(&endpoints, &options);
    get_images(&endpoints, &options);
    let scan_jobs = set_up_scan_jobs(&endpoints, &options);
    println!("SCAN JOBS: {:?}", scan_jobs);
    println!("LENGTH OF SCAN JOBS: {}", scan_jobs.len());
    //scan_images(scan_jobs[..1].to_vec(), &options);



}