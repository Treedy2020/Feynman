# Keep governance gates in a provider-agnostic domain core

Feynman V1 keeps the Project, Evidence, Readiness, Review, Capability, Deployment and Self Repo gates in a deterministic TypeScript domain core, while Google Cloud, model providers and durable storage are adapters. This lets the safety and audit invariants be tested without credentials or network state; provider adapters must satisfy the existing interfaces and cannot bypass the gates.

## Consequences

- The local MVP uses in-memory storage and a successful deployment adapter.
- Cloud Build, Artifact Registry, Cloud Run, LLM reasoning, authentication and durable repositories still need production adapters.
- A provider integration failure cannot silently redefine the Project workflow or capability envelope.
