# Private HTTPS providers and custom CA trust

AI Cost Firewall supports **additional private certificate authorities** for
HTTPS upstream LLMs and embedding providers. This includes OpenShift
service-CA-signed endpoints. The CA roots are added to the default WebPKI roots;
AIF does **not** turn off chain verification or hostname/SAN validation.
Observe mode and Enforce mode use the same live-provider TLS configuration.

## Configure the two provider clients

In `ai-firewall.conf`, set either or both directives as needed:

```nginx
upstream_base_url https://chat.src-llm.svc.cluster.local:8443/v1;
upstream_ca_bundle /etc/aif-certs/service-ca.crt;

embedding_base_url https://embeddings.src-llm.svc.cluster.local:8443/v1;
embedding_ca_bundle /etc/aif-certs/service-ca.crt;
```

The first bundle covers **buffered chat, `/v1/models` discovery, and controlled
streaming**. The second covers embeddings. Paths are independent; they may
reference the same PEM certificate file or different internal roots. **No bundle
configured** means normal built-in WebPKI roots only.

When running **without** `ai-firewall.conf`, use:

```sh
AIF_UPSTREAM_CA_BUNDLE=/etc/aif-certs/service-ca.crt
AIF_EMBEDDING_CA_BUNDLE=/etc/aif-certs/service-ca.crt
```

AIF's current `reqwest`/rustls provider clients do not load the custom service
CA merely because `SSL_CERT_FILE` is set. Use the explicit AIF directives or
environment variables. No additional certificate configuration is necessary
for an HTTPS endpoint already trusted by the built-in WebPKI roots.

The mount path must be readable by AIF's runtime UID. The PEM may include
multiple CA certificates. If configured and unreadable, empty, or invalid,
AIF reports an error rather than silently disabling TLS verification. The
bundle is **server trust only**, not a client certificate or mutual-TLS (mTLS)
client identity. Other connections (Redis, Qdrant, Audit, Guards) do not inherit
these CA directives.

## OpenShift service CA injection

Create a ConfigMap in the AIF namespace with the injection annotation:

```yaml
apiVersion: v1
kind: ConfigMap
metadata:
  name: aif-service-ca
  namespace: aif-pilot
  annotations:
    service.beta.openshift.io/inject-cabundle: "true"
```

Apply it, and allow the OpenShift service CA controller to populate the
`service-ca.crt` key. Confirm it is populated before starting AIF:

```bash
oc -n aif-pilot get configmap aif-service-ca \
  -o jsonpath='{.data.service-ca\.crt}' | grep -m1 'BEGIN CERTIFICATE'
```

Mount it read-only into the AIF container; the following is a **fragment** to
merge into the existing AIF Deployment (not a standalone manifest):

```yaml
spec:
  template:
    spec:
      volumes:
        - name: aif-service-ca
          configMap:
            name: aif-service-ca
            items:
              - key: service-ca.crt
                path: service-ca.crt
      containers:
        - name: ai-firewall  # replace with the actual container name
          volumeMounts:
            - name: aif-service-ca
              mountPath: /etc/aif-certs
              readOnly: true
```

The upstream and embedding service certificates must be issued by a CA in this
bundle **and** have SANs matching the hostnames configured in their base URLs.
The injected service CA is not an application secret, but trust roots must be
controlled by administrators: installing an unexpected CA expands the set of
servers AIF will accept. No private server keys are mounted into AIF.

## Verification

1. Confirm the file is present and readable *inside the AIF container*.
2. Test each HTTPS service directly with the **same DNS name** used by AIF and
   an explicit bundle (do not use `curl -k`):

   ```bash
   curl -v --cacert /etc/aif-certs/service-ca.crt \
     https://chat.src-llm.svc.cluster.local:8443/v1/models
   curl -v --cacert /etc/aif-certs/service-ca.crt \
     https://embeddings.src-llm.svc.cluster.local:8443/v1/models
   ```

   A `404` from a provider that lacks `/v1/models` can still demonstrate TLS
   verification when the verbose output confirms the handshake succeeded.
3. Restart the AIF pod and verify startup INFO messages:
   `loaded custom upstream CA roots for buffered and streaming clients` and
   `loaded custom CA roots for embedding client`.
4. Submit **non-streaming** and **streaming** chat requests through AIF.
   Exercise an uncached prompt with semantic evaluation enabled to verify the
   embedding path; simply receiving a chat response does not establish that
   embedding HTTPS works in Observe mode.
5. Confirm that `upstream_tls_error` no longer occurs for either provider.
   Untrusted CAs and SAN/hostname mismatches must still be rejected. Never
   disable HTTPS or certificate validation to hide the error.

## CA changes, reload, and rotation

AIF loads the CA bundles when constructing its HTTP clients. CA **path changes**
are restart-only and rejected on SIGHUP. Updating the bytes of a mounted CA
file (for example after OpenShift service CA rotation) does not update an
existing client's trust store: **restart the AIF pod** so buffered, streaming,
and embedding clients all load the same current roots.

If you see `Client auth requested but no cert/sigscheme available` in `rustls`
logs, the server requested a client certificate. That does *not* by itself mean
mTLS is required. This feature does not supply a client certificate. Confirm
server expectations separately if the handshake still fails with a trusted CA.

Also see [Configuration Reference](config-reference.md),
[Deployment](deployment.md), and [TLS Troubleshooting](troubleshooting.md).
