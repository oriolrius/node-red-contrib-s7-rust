# Security policy

## Supported versions

Only the latest minor release gets security fixes.

## Report a vulnerability

Do not open a public issue. Use GitHub private vulnerability reporting:
<https://github.com/oriolrius/node-red-contrib-s7-rust/security/advisories/new>
(**Security → Report a vulnerability** in this repository). You get an answer within 7 days.

Published advisories: <https://github.com/oriolrius/node-red-contrib-s7-rust/security/advisories>.

Include the version, the PLC model and firmware, and a capture or the steps to reproduce.
We fix confirmed issues in a new release and credit the reporter in the advisory, unless the
reporter asks us not to.

## Scope notes

- The nodes connect to PLCs over S7comm (no authentication in the protocol) and S7CommPlus.
  Keep PLC networks separate from untrusted networks.
- S7CommPlus TLS does not verify the PLC certificate.
- The admin API routes use the Node-RED admin authentication (permission `s7rs-endpoint.read`)
  and allow read-only actions only.
