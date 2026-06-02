# Taste (Continuously Learned by [CommandCode][cmd])

[cmd]: https://commandcode.ai/

# Workflow
- Read the entire codebase thoroughly before writing modernization plans or architectural proposals. User emphasized this twice as critical. Confidence: 0.90
- Check existing implementations in the codebase before proposing new designs — features like VNet, subnet, NSG, route tables, and leader election may already be implemented and tested. Confidence: 0.85
- Run tests with sudo. Tests involving ImageManager require root permissions to pass. Confidence: 0.65

# Architecture
- Prefer standard Kubernetes YAML and annotations over custom `x-z8s-*` extension fields. User explicitly stated custom extensions are not their preference. Confidence: 0.85
- Use standard K8s annotations (e.g., `z8s.io/*`) for z8s-specific features rather than inventing new spec fields or extension patterns. Confidence: 0.80
- RBAC should control what pods (via service accounts) can do to cluster APIs — enabling pods to run commands like "get pods", "get services", "apply", etc. Not just human user access. Confidence: 0.75
- For facade pattern: Make the main struct (e.g., `NetMux`) BE the facade itself — the single entry point all components use. Don't create separate "Facade" structs; internal implementations (e.g., `NftEngine`) should be hidden implementation details. Confidence: 0.80

# Testing
- Use kubectl commands instead of curl for test scripts. User explicitly corrected curl-based test to use kubectl. Confidence: 0.80
