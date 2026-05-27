can a deployment has pods on multi-node ?
if the pod crashed a new pod will be created, will it get new ip or use the old one, and what if a pod keeps crashing will be reach end of ips for its namespace?
does pods of same deployment can see each others and do they in same ip range, even if they distrubited around multi-node?
can our reverse proxy work for private vnets / subnets so we can have ingress by domain but accepts only from another subnet / vent so we can have truly hub and spoke ? so we might have a subdomain for the db service liiving in A1 subnet and another service lives in subnet A2 and a home service lives in A3 subnet and home service is the hub and others are spoke and home service want to access db on db.myapp and we need to route this to the db on A1 subnet. and this route is only supoorted to be handled from the hub to spoke and not verse versa. so we have our reverse dns can even work for private nets / subnets which doesnt face internet and maybe the hub only has internet access. (does my question right?) 
can our plan support a node to host apps and another node has the db and so on, so the requests get routed fine and could be streaming, high throuput data.
I think Node Discovery is required.
I beleive phase 0 should be creating tests (senario scripts with yaml content) so we track our progress and achivements.
can we avoid Libc FFI? does rustables can cover us?
since NSG conflict with NetworkPolicy and More specific rule wins by evaluation order , i would like if we start by only supporting NSG and make it cover same NetworkPolicy stuff. but i want NSG to handle IPs, subnets, domain and subdomains, and wildcard domains, ports, protocols, intranet, internet, custom headers in http/s requests, method type (get,post,..) like a firewall. 
Does this plan supports tls like https or it can be restricted to http?
I have some servers which has IPv6 /64 IPs. its own IP list, with a single IPv4. In our plan we should support assigning an IPv6 or IPv4 to resource (ingress) or to a node or what is the best here? imagine azure when it assign public ip for resource like loadbalancer of so on, we need to be able to do so exactly. 

