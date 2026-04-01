# Security Policy

## Reporting a Vulnerability

If you discover a security vulnerability, please report it responsibly:

1. **Do not** open a public GitHub issue
2. Email: [open an issue with the `security` label as a placeholder for now]
3. Include: description, reproduction steps, potential impact

We will acknowledge receipt within 48 hours and provide a timeline for a fix.

## Scope

quant-cache is a cache optimization engine that processes trace data offline.
It does not:
- Handle authentication or user credentials
- Connect to external services in V1
- Process untrusted input in production (traces are operator-provided)

Security considerations for future versions (V2.5+) that connect to CDN provider APIs
will be documented separately.
