# AIF v0.8 Evaluation Mode Validation Report

**Product:** AI Cost Firewall (AIF)  
**Version under test:** v0.8.0  
**Validation scope:** Observe Mode prediction vs Enforce Mode realization  
**Test duration:** 5 minutes per mode  
**Target workload:** 3.0 requests/second  
**Document status:** VCAL technical validation evidence  
**Date:** 2026-09-27

---

## 1. Executive Summary

This validation tests whether **AIF Observe Mode provides a representative prediction of the cache, policy, token, and cost outcomes subsequently produced when the same controls are enabled in Enforce Mode**.

Two full-stack runs were executed against the same workload profile:

1. **Observe Mode** - AIF evaluated cache and control decisions without allowing evaluation cache hits to suppress normal upstream execution.
2. **Enforce Mode** - AIF applied production cache and control decisions, allowing cache hits and policy actions to affect request processing.

The results show close agreement between the outcomes predicted in Observe Mode and those realized in Enforce Mode. Observe recorded **720 hypothetical exact-cache hits** and **1 hypothetical semantic-cache hit**, while Enforce realized **721 exact-cache hits** and **1 semantic-cache hit**. Security, Privacy, and Usage Guard action counts matched in the validated workload. Predicted and realized token/cost savings were similarly close.

The deliberately different result was upstream execution. Observe Mode continued to call the upstream provider after shadow cache decisions, whereas Enforce Mode used real cache hits to avoid those calls. This is the intended distinction between non-disruptive evaluation and production enforcement.

**Validation conclusion:** under this controlled workload, the test provides strong product-generated evidence that AIF v0.8 Observe Mode can estimate outcomes that are subsequently realized when Enforce Mode is enabled, within normal run-to-run variance.

This report is **VCAL-generated technical validation evidence**, not an independent benchmark, third-party certification, or guarantee that every production workload will produce identical Observe and Enforce results.

---

## 2. Validation Objective

The experiment tests the following proposition:

> For the same representative workload and policy configuration, AIF Observe Mode should predict the cache, guard, token, and economic outcomes that are subsequently realized when AIF is switched to Enforce Mode.

### Metrics expected to be approximately equivalent

- Exact-cache opportunity vs actual exact-cache hits
- Semantic-cache opportunity vs actual semantic-cache hits
- Security Guard decisions
- Privacy Guard transformations
- Usage Guard policy stops
- Avoidable vs actually saved tokens
- Hypothetical vs realized cost savings

Small differences are expected because the runs are independent executions and can differ slightly in request count, timing, cache evolution, concurrency, and measurement boundaries.

### Metric expected to differ

**Upstream provider calls are intentionally not expected to match.**

In Observe Mode, a shadow cache hit records that an upstream call *could* have been avoided, but normal upstream execution continues. In Enforce Mode, an actual cache hit serves the cached response and suppresses the corresponding upstream request.

This expected difference is central to the validation: **Observe measures the intervention; Enforce applies it.**

---

## 3. Test Methodology

Both runs used the same full-stack validation harness and target workload:

- Duration: **300 seconds**
- Target request rate: **3.0 RPS**
- Target requests: approximately **900**
- Workload composition:
  - 45% exact-cache workload
  - 30% semantic-cache workload
  - 10% Privacy Guard workload
  - 5% direct/upstream workload
  - 5% Security Guard workload
  - 5% Usage Guard workload

The runs exercised AIF together with the relevant VCAL modules and supporting services, including cache dependencies, guard modules, Audit, Compliance, Console, and the controlled test upstream.

### Observe run

Observe Mode used the isolated evaluation/shadow cache scope. Cache decisions were measured without allowing shadow hits to suppress upstream execution.

Measured workload: **847 requests at 2.823 RPS**.

### Enforce run

Enforce Mode used the production cache scope and applied cache/control decisions.

Measured workload: **848 requests at 2.827 RPS**.

The Enforce validation completed with:

- **194 passed**
- **4 warnings**
- **0 failed**
- **Full-stack verification: PASSED**

The warnings were non-blocking test-profile conditions rather than failures of the core Observe/Enforce behavior.

---

## 4. Observe-to-Enforce Results

| Measurement | Observe Mode | Enforce Mode | Comparison |
|---|---:|---:|---|
| Workload requests | 847 | 848 | 1 request difference |
| Actual RPS | 2.823 | 2.827 | 0.14% delta |
| Exact cache hits | 720 would-hit | 721 actual | 0.14% delta |
| Semantic cache hits | 1 would-hit | 1 actual | Exact parity |
| Security request blocks | 42 | 42 | Exact parity |
| Privacy transformations | 84 | 84 | Exact parity |
| Usage policy stops | 42 | 42 | Exact parity |
| Tokens avoidable/saved | 1,564,581 potential | 1,565,346 actual | 0.05% delta |
| Net savings | $0.292944 potential | $0.2930 actual | 0.02% delta |
| Upstream calls | 763 | 42 | Expected difference by design |

### Key result

The cache prediction was particularly close:

- Observe: **720 exact would-hits + 1 semantic would-hit**
- Enforce: **721 actual exact hits + 1 actual semantic hit**

The exact-cache difference was approximately **0.14%**.

Token economics were similarly close:

- Observe potential token avoidance: **1,564,581**
- Enforce actual token savings: **1,565,346**
- Difference: approximately **0.05%**

The net-savings estimates were also effectively equivalent for this workload.

---

## 5. Prediction to Realization

```text
                    SAME REPRESENTATIVE WORKLOAD
                              |
                 +------------+------------+
                 |                         |
            OBSERVE MODE              ENFORCE MODE
                 |                         |
          Shadow decisions           Applied decisions
                 |                         |
       720 exact would-hits  ------>   721 actual exact hits
         1 semantic would-hit ------>     1 actual semantic hit
        42 security decisions ------>    42 security blocks
        84 privacy actions   ------>    84 privacy transformations
        42 usage decisions   ------>    42 usage policy stops
       ~$0.293 potential saving ------> ~$0.293 realized saving

        Upstream continues             Cache hits suppress
          by design                    upstream execution
```

**Observe measures the intervention. Enforce applies it.**

---

## 6. Upstream-Call Behavior

The upstream-call comparison demonstrates the intended architectural distinction.

### Observe Mode

Observe Mode identified **721 cache-hit opportunities**, but shadow cache hits did not suppress normal upstream processing. The run therefore recorded **763 upstream calls**.

### Enforce Mode

Enforce Mode realized **722 cache hits** and recorded only **42 upstream calls** for the measured demo workload.

This is not an Observe/Enforce mismatch. It is the expected consequence of switching from measurement to application of cache decisions.

---

## 7. Guard Decision Validation

### Security Guard

Observe and Enforce each produced **42 Security request blocks** in the validated workload.

### Privacy Guard

Observe and the Enforce run each recorded **84 Privacy transformations**. Observe recorded 805 Privacy scans and Enforce recorded 806; the one-scan difference tracks the one-request difference between the workloads.

### Usage Guard

Observe and Enforce each produced **42 Usage policy stops**. Observe recorded 805 Usage scans and Enforce recorded 806.

These results support using Observe Mode to estimate policy impact before enforcement is enabled.

---

## 8. Controlled Streaming and Cache Reuse

The full-stack Enforce validation also exercised controlled-streaming cache behavior.

The test verified cross-format cache reuse in both directions:

- a completion generated through streaming could subsequently satisfy a JSON request from exact cache without another upstream call;
- a JSON completion could subsequently satisfy a streaming request from exact cache without another upstream call.

The validation also exercised the controlled-streaming commit barrier under an injected upstream failure, checking that a failed upstream response did not leak provider canary content or incorrectly commit a successful downstream response.

These checks provide additional evidence that the cache behavior evaluated by v0.8 remains compatible with AIF's controlled-streaming architecture.

---

## 9. Audit and Compliance Evidence

The full-stack validation included VCAL Audit and VCAL Compliance behavior rather than testing AIF metrics in isolation.

The harness exercised:

- request/evidence trace generation;
- guard-related evidence;
- cache-decision evidence;
- Audit hash-chain verification;
- Compliance import and synchronization after the workload.

This matters for customer evaluation because an Observe assessment can be supported by traceable evidence rather than relying only on dashboard totals.

---

## 10. Limitations

1. **This is VCAL-generated evidence.** It is not an independent benchmark or third-party certification.
2. **The principal comparison consists of controlled five-minute runs.** It demonstrates close Observe/Enforce agreement for the tested workload but does not establish a universal error bound.
3. **Observe and Enforce are separate executions.** Small differences can result from request timing, concurrency, cache evolution, and measurement boundaries.
4. **The workload is controlled.** Customer traffic may have different repetition rates, semantic similarity, response sizes, models, prices, and guard findings.
5. **Savings depend on workload and provider economics.** The dollar values measured here are not a forecast for a different customer workload.
6. **Upstream-call parity is intentionally not expected.** Observe continues normal upstream execution after shadow decisions; Enforce can suppress upstream calls through actual cache hits.

---

## 11. Recommended Extended Validation

For stronger statistical evidence, repeat:

- Observe Mode: **5 independent 5-minute trials**
- Enforce Mode: **5 independent 5-minute trials**

For each parity metric, report:

- Observe mean and standard deviation;
- Enforce mean and standard deviation;
- absolute difference;
- percentage difference.

A second guard-heavy workload profile can complement the current cache-heavy profile.

This would characterize Observe-to-Enforce agreement across repeated trials rather than relying on a single paired demonstration.

---

## 12. Conclusion

For the tested representative workload, **AIF v0.8.0 Observe Mode's predicted cache, control, token, and cost outcomes closely corresponded to the outcomes subsequently realized in Enforce Mode**.

The principal measured relationships were:

- **720** hypothetical exact-cache hits vs **721** realized exact-cache hits;
- **1** hypothetical semantic-cache hit vs **1** realized semantic-cache hit;
- **42** Security request blocks vs **42** realized blocks;
- **84** Privacy transformations vs **84** realized transformations;
- **42** Usage policy stops vs **42** realized policy stops;
- **1,564,581** potentially avoidable tokens vs **1,565,346** actually saved tokens;
- approximately **$0.293** potential net savings vs approximately **$0.293** realized net savings.

At the same time, the large difference in upstream calls demonstrates the intended evaluation property: **Observe Mode measures what AIF would do without applying cache-hit suppression, while Enforce Mode applies those decisions.**

For prospective customers, this supports a deployment model in which AIF can first run in Observe Mode to quantify expected savings and policy impact, the customer can review the resulting evidence, and approved controls can then be moved to Enforce Mode and compared against the original prediction.

---

## Evidence Files

This report was prepared from:

- `observe-5min.log`
- `enforce-5min.log`

**AI Cost Firewall v0.8.0 - Evaluation Mode Validation**
