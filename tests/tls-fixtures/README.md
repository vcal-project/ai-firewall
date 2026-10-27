# Local TLS regression fixtures

These files contain **test-only, public/insecure key material**, generated specifically for loopback TLS tests. Never reuse these certificates or the server private key in a real deployment. The server certificate contains `DNS:localhost` as its sole SAN; connections via `127.0.0.1` intentionally fail hostname verification. The CA is not included in WebPKI roots. The test server binds to `127.0.0.1` on an ephemeral port.

Fixtures: `ca.pem` (trust root), `server.der` (server certificate), `server-key.der` (unencrypted PKCS#8 private key for the local test server).
