lwproxy command line utility
===

A command line utility for interfacing with lwproxy in scheduling container image assurance scans in JFrog registries.<br/><br/>
Current Status: Experimental<br/><br/>

## Example Usage

    lwp --registries test.jfrog.io:443[username:${PASSWORD}],test2.jfrog.io:443[username:${ENCRYPTED_PASSWORD}]  \ 
        --repo-keys default-docker-local --proxy-scanner-address http://192.168.1.41:9080 --num-images 3 --scans-hour 1500

<br/><br/>
lwp -h


<br/><br/>
--num-images is the number of images per image repository by created date<br/><br/>
For detailed output set env variable RUST_LOG=info
 
