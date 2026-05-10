# OpenAI-compatible providers

AI Cost Firewall supports practical OpenAI-compatible HTTP providers without provider-specific config blocks.

## Supported provider type

```text
upstream_provider openai_compatible;
embedding_provider openai_compatible;
```

## Base URL format

Use either the provider root URL or the `/v1` base path.

Correct:

```conf
https://api.openai.com
https://api.openai.com/v1
http://ollama:11434
http://ollama:11434/v1
```

Incorrect:

```conf
http://ollama:11434/v1/chat/completions
http://ollama:11434/v1/embeddings
```

## Local providers without authentication

Use:

```conf
upstream_api_key dummy;
embedding_api_key dummy;
```

AI Cost Firewall will skip the upstream Bearer token for placeholder keys.

## Separate chat and embedding endpoints

```conf
upstream_base_url http://ollama:11434/v1;
embedding_base_url http://embedding-gateway:8080/v1;
```

## Examples

See

```text
configs/examples/openai.conf
configs/examples/ollama-openai-compatible.conf
configs/examples/lm-studio.conf
configs/examples/vllm.conf
configs/examples/litellm.conf
configs/examples/openrouter.conf
```

