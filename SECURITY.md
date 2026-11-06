# Security policy

## Supported versions

Only the latest minor release gets security fixes.

## Report a vulnerability

Do not open a public issue. Use GitHub private vulnerability reporting:
**Security → Report a vulnerability** in this repository. You get an answer within 7 days.

## Scope notes

- The nodes connect to PLCs over S7comm (no authentication in the protocol) and S7CommPlus.
  Keep PLC networks separate from untrusted networks.
- S7CommPlus TLS does not verify the PLC certificate.
- The admin API routes use the Node-RED admin authentication (permission `s7rs-endpoint.read`)
  and allow read-only actions only.
