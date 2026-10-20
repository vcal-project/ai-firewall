# Structured Message Content and Privacy-Aware Caching

AIF parses message `content` as JSON, preserving OpenAI-style content-part arrays and other non-string
content rather than flattening them. **Preserving structure does not mean that every part is inspected by
every Guard.**

## Request shape and cache eligibility

- Exact cache identity includes complete JSON message content and completion-affecting top-level/message
  extension fields, including tools, tool choice, tool-call IDs, reasoning parameters, and response-format
  fields.
- Requests with **non-string** message content are not eligible for semantic cache lookup/store. Eligible
  structured-content requests may still use exact caching.
- JSON versus controlled-SSE delivery does not change the eligible request's cache identity.

## Privacy Guard inspection scope

When a compatible VCAL Privacy Guard is enabled, AIF sends supported strings, arrays of strings, and typed
text parts (such as `{"type":"text","text":"..."}`) for inspection without flattening the array. Supported
mixed text/non-text arrays preserve their shape.

Recognized unsupported **text-bearing** shapes are rejected as a privacy inspection contract violation
rather than silently forwarded as inspected. Opaque non-text parts may pass through but **are not scanned
for embedded information**. `guard_fail_open` governs operational Guard failures, not the unsupported-text
safety contract.

### Security Guard and Usage Guard

Privacy Guard's structured-content integration does **not** establish inspection coverage for the other
Guards. Security Guard and Usage Guard have independent adapter/API contracts, and their coverage must be
verified for the **actual AIF and module versions deployed**. In particular, do not infer that an arbitrary
nested content-part array has been inspected simply because it was preserved by AIF.

This caveat is important when upgrading Guard integrations: refer to the version-specific [upgrade
notes](upgrading.md) and tests for the deployed release. No universal all-shapes inspection guarantee should
be inferred for either Guard.

## Privacy-aware cache isolation and restoration

When Privacy Guard provides a complete effective policy ID, version, and hash, AIF derives an opaque cache
scope from the effective tenant and policy identity. Exact-cache identity and Qdrant semantic filters
separate entries across tenant/policy scopes.

AIF stores cached completions in the **pre-restoration placeholder** form, and restores placeholders using
only the mapping for the **current request**. If a successful Privacy scan lacks a complete policy identity,
or if a Privacy Guard transport failure is allowed to fail open, AIF bypasses both cache lookup and storage
for that request.

## Attachments are outside inspection scope

This is **not** document or attachment processing. AIF does not fetch referenced files, decode arbitrary
opaque image/document parts, or extract document text. Applications that need inspection of an attachment
must supply extracted text through an explicitly supported text-bearing path and validate the applicable
Guard contracts.

See [How It Works](how-it-works.md) for the cache decision path, [Provider
Compatibility](provider-compatibility.md) for upstream API limitations, and [Operations](operation.md) for
failure policies.
