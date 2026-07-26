# Model providers

## Local models

`purrcode init` discovers:

- Ollama at `127.0.0.1:11434`
- LM Studio/OpenAI-compatible servers at `127.0.0.1:1234`

Local-only mode rejects every provider not declared local and loopback. It never silently falls
back to remote inference.

## OpenAI and remote gateways

Store credentials with hidden input:

```bash
purrcode credential set openai
```

The key is held by the operating-system credential store. Configuration contains only a
`keychain:openai` reference. Environment-variable references are supported for ephemeral CI.
Credentials are added only to provider HTTP requests and are scrubbed from tool/plugin children.

Enterprise gateways additionally support mTLS identity PEMs, custom CAs, proxies, bounded
credential commands, and secret-backed headers.

## Models and roles

```bash
purrcode model add openai/gpt-codex
purrcode model use openai/gpt-codex
purrcode model list
purrcode model qualify openai/gpt-codex
purrcode provider doctor
```

Daemon sessions require a `judge` role. Coder and judge must differ unless configuration explicitly
accepts reduced independence. Qualification tests structured output, coding/judgment cases,
latency, throughput, and reliable context before recommending roles.
