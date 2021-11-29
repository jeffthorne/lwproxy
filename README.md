lwproxy command line utility
===

A command line utility for interfacing with lwproxy in scheduling container image assurance scans in JFrog registries.<br/><br/>
Current Status: Experimental<br/><br/>

## Example Usage

lwp --registries test.jfrog.io:443[username:${PASSWORD}],test2.jfrog.io:443[username:${ENCRYPTED_PASSWORD}]  \ <br/>
    --repo-keys default-docker-local --proxy-scanner-address http://192.168.1.41:9080 --num-images 3

<br/><br/>
lwp -h


<br/><br/>
For detailed output set env variable RUST_LOG=info
 
