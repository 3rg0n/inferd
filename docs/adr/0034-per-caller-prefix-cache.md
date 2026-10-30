# 0034. KV cache managed like memory: per-caller address spaces, quotas and an advertised context budget

- Status: proposed
- Date: 2026-10-30

## Context

The llamacpp backend clears the KV cache after every request
(`llama_memory_clear`, module docs of `llamacpp/backend.rs`). Agent
middleware resends the system prompt, tool declarations and the whole
conversation each turn, so every turn re-reads a prefix that was in the
cache moments ago. Measured on a CPU-only Core Ultra 7 165U with
gemma-4-e4b (#78): prompts grow from ~1.1k to ~2.7k tokens, prompt
evaluation runs at ~33 tok/s, and 50-70 s of each 60-80 s turn is
re-reading; only 100-300 tokens per turn are new.

Clearing per request has one property worth keeping: no request ever
sees state another caller left behind. Several middlewares share one
warm model (ADR 0012); one must never be able to see, match, time or
crowd out another's context — by accident or otherwise.

Callers also have no way to learn how much context they may use. bofh,
for example, hard-codes a 6,000-token budget to sit under inferd's
default `n_ctx` of 8,192; nothing tells it when that changes.

`swa_full` defaults to true in the vendored llama.cpp, so Gemma 4's
sliding-window layers keep full KV and a prefix can be reused.

## Decision

Treat the KV cache the way an OS treats RAM.

1. **Working context = the CPU.** One generation runs at a time (ADR
   0012), and it gets the whole `n_ctx`. Context is time-sliced, not
   split: a caller's usable window does not shrink because others
   exist.

2. **Retained cache = per-process memory, in separate address spaces.**
   After a request, inferd may keep that caller's evaluated tokens and a
   KV snapshot (`llama_state_seq_get_data`) so the next request from
   the same caller restores it, finds the longest common token prefix,
   drops the tail from the divergence point and evaluates only the rest.
   - **Owner** = the kernel identity inferd already sees per connection:
     SID/uid **plus** process id **plus** process start time (pid alone
     is reused).
   - Lookup is confined to the owner's space. There is no
     content-addressed sharing between callers, so no cross-caller hit,
     no cross-caller timing signal, nothing to consume by accident. (A
     deliberate, opt-in shared prefix — the analogue of a shared memory
     segment — is out of scope until someone needs it.)
   - A caller holding several conversations may pass a `cache_key`; it
     names entries only inside its own space.

3. **Quotas, so one caller can't starve the rest.** The retained-cache
   pool is a share of the ADR 0032 memory budget. Each active caller is
   guaranteed `pool / active_callers` and may borrow idle space above
   that. Under pressure inferd evicts, in order: entries past their TTL,
   then the over-quota caller's own least-recently-used entries, then
   anyone's beyond their guarantee. A caller's guaranteed share is never
   taken to serve another.

4. **Paging policy like Bedrock.** An entry lives 5 minutes from last
   use, refreshed on every hit; nothing below 1,024 prefix tokens is
   stored; a caller's entries are freed as soon as its process exits.
   Media requests and model changes bypass the cache.

5. **Upfront, advertised budget.** The capabilities frame each caller
   already receives gains, for that caller: `context_tokens` (`n_ctx`),
   `max_output_tokens`, `cache_quota_bytes` and `cache_ttl_secs`.
   `usage` gains `cache_read_tokens` / `cache_write_tokens`. Middleware
   sizes its prompts from these numbers instead of hard-coding them, and
   can see whether it is cache-friendly.

6. **Middleware's part (documented, not enforced):** keep the system
   prompt and tool declarations byte-identical and first, keep history
   append-only, and compact rarely in one block rather than trimming
   from the middle — any change early in the prompt is a full miss.

## Consequences

**Easier:** agent turns cost roughly (new tokens / prompt rate) +
(output / generation rate) — ~10-15 s instead of 60-80 s on the host
above; callers that do nothing behave as today; a noisy caller cannot
evict a quiet one's guaranteed share.

**Harder / costs:** snapshots cost host memory (inside the ADR 0032
budget); restore adds a copy per request; the entry table and quota
accounting need care under concurrent requests.

**Isolation, stated precisely:** inferd never returns, matches, times or
evicts-for-gain another caller's cached state. It does not protect
against hostile code already running as the same OS user, which can
read the user's processes' memory directly — the same boundary inferd's
peer-credential check draws.

**Explicitly NOT changed:** one warm model, one active generation at a
time, the admission queue.

## References

- Issue #78; ADR 0012, ADR 0032.
- Amazon Bedrock prompt caching (5-minute TTL refreshed on hit).
- llama.cpp server `cache_prompt` (longest-common-prefix reuse).
