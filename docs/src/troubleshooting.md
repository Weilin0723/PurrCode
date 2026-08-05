<p align="center">
  <a href="README.md"><img src="purrcode-logo.png" alt="PurrCode" width="180"></a>
</p>

# Troubleshooting

## Environment diagnostics

```bash
purrcode doctor --repository "$PWD"
```

`doctor` performs bounded, read-only project and host detection, records real version probes, and returns one evidence-bearing plan with detected, missing, install, and verification records. A missing tool is reported explicitly — never as installed or ready.

```bash
purrcode sandbox doctor
```

Reports the host's real isolation capability. Degraded isolation is reported accurately and is not represented as a full sandbox.

```bash
purrcode provider doctor
```

Classifies provider failures — connection refused, DNS failure, TLS failure, authentication failure, HTTP error, content-type mismatch, incompatible schema, streaming framing failure, unsupported API mode, model not found, context too large, out of memory, cancellation.

## Known limitations

PurrCode is under active development. Before using it with sensitive repositories, review:

- current implementation status;
- host sandbox capability;
- provider qualification results;
- release acceptance evidence;
- platform-specific limitations.

Important considerations:

- not every provider has been live-tested on every platform;
- some model capabilities remain unknown until qualification;
- weak host isolation cannot provide the same guarantees as a supported OS sandbox;
- local model quality and memory requirements vary by model and quantization;
- reduced-independence mode may use the same model for proposal and judgment;
- external skills and MCP servers remain untrusted until qualified;
- a successful unit test suite does not replace testing against your exact provider and environment.

## Where to go next

- The repository's [troubleshooting guide](https://github.com/Weilin0723/PurrCode/blob/main/docs/troubleshooting.md)
- The [implementation status](https://github.com/Weilin0723/PurrCode/blob/main/docs/implementation-status.md) for verified capabilities and remaining gates
- Open an issue at https://github.com/Weilin0723/PurrCode/issues
