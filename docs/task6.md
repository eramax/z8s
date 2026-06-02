read everyfile in ./src

I want a comprehensive refactoring and rewriting and rearchitecute and modernize and improvment with new features plan for this project. 
your goal is to make it muture product for production, much better than k3s and s6. with cloud features like vnet, subnets, ngs, loadbalalncer,support assigning public IP (from the host subnets - i have a server with ipv6 /64) and much more faster and less blocking. better networking (current code is a mess, i want a clean code for managing the nft chains.) , cri (needs to use overlays efficient). with better architecture design and clean and decoupled components or better dependancy flow design.
the current schedular is slow for two nodes, it is about 5x the time for single node, while mutli-node should be doubled the speed not 5x slower, we have to fix that.
also the cloc is 19451, we need to go extremly less e.g, 5000 by writing clean and optimized code by utlizing generics , functions, configuration based, reusable components, composition, adapte the best design patterns for each case e.g, pipelines, builder which i see they are great here.
the z8s should be much more faster, lightweight, utlize async and maybe io_urings. less dependancies.
new features are welcomed, so i can be a complete cloud infrastracture mutli-node with support for most cloud native apps needs e.g, azure,aws we are using same k8s yamls but we can extends it by more cdrs. but some features can be combined i think like service, loadbalancer,apigateway, routetable so we are deliver more features by same cdr to reduce development complexity, we are free to extend the standard cdrs.
true isolation is very important, form the network, storage (disks), and resource limits. we inforce all.
all pods should be living in a default vent if they didnt set a vnet to use so we have a unified design.
support running as PID 1 with multi-core systems. never crashes or get down. and can run normal processes in the host like bash commands, dhcp, sshd, etc. all, we need to know what capabilities each app will need and support that capabilities. 
we are lacking roles. do we need it ?
checout this plan as well maybe it can give u some ideas.
write your plan in a md file in chunks to avoid output tokens limits.