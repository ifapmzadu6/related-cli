# Agent guardrail follow-up — 2026-08-16

This targeted run repeated the pilot's failed native-header-icon task with
`related-cli@0.4.0` and the proposed skill guardrails. The agent still ran the
same related-file query first, then had to search every explicit task target,
use additional anchors for independent surfaces, and let the task text override
the ranking.

## Result

| Run | Task success | File precision | File recall | Changed-line precision | Changed-line recall | Non-cached input | Time |
|---|---:|---:|---:|---:|---:|---:|---:|
| Original treatment | no | 50% | 50% | 50% | 50% | 52,871 | 144.6s |
| Guarded follow-up | yes | 100% | 100% | 100% | 100% | 56,151 | 147.0s |

The guarded run changed exactly the four requested files: the iOS and Android
settings screens plus the iOS and Android notification-history screens. Its
eight added/deleted lines exactly matched the historical patch, and
`git diff --check` passed.

Run command:

```sh
python3 scripts/agent_ab.py \
  --repo /path/to/too_tired_to_type \
  --cases experiments/agent-ab/too-tired-to-type.json \
  --output /tmp/related-agent-ab-guardrail \
  --case native-header-icon-size \
  --arm with-related
```

## Interpretation

This is one non-deterministic follow-up, not a paired rerun or a general causal
result. It shows that the proposed instructions can prevent the specific
over-trust failure observed in the pilot. It does not establish that guarded
`related-cli` use improves agent accuracy across tasks or repositories.
