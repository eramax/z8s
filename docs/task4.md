You are a professional rust developer and architect. we want the best for this task, something mantinable, extendable, and robust. we dont want to break anything.
I would like to refactor the project and restructure it to be like this
- api dir
- components dir
  - Pod.rs
  - Deployment.rs
  ....
- cri dir
- net dir
- schedular dir
the reason we are going to add more components like Vnet and NSG and so on and i want the system is capable to add more components and these components are standalone and our solution architecture plug the component and apply it to the flow (execution follow, eg. network policy). 
and this is for making our src code ready for our new network engine and components discribed at /home/abb/dev/z8s/docs/network-architecture-plan.md

also i want to utilize generics and code reuse for the components for example we have 
- Compute resources: Pod, Deployment, Job
- Network resources: Service, VNet, NSG, NetworkPolicy
- Storage resources: PV, PVC, ConfigMap, Secret

my idea is we have a good decoupling between network engine (netmux) and our cri and our api server and by our api server it uses the scheduler which uses both the cri and netmux and all are decoupled. and clean. (i dont want to ship each as a seperate package but at least they can get seprated if we need since no coupling. 

what do u think ? 

