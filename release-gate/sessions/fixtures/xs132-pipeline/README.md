# CPU caller of BiRefNet

The orchestrator calls BiRefNet, an ordinary dependency from the Hub's root index, through its generated request and
`Operation` reference. Regenerate from this directory with BiRefNet locked:

```sh
cozy-runtime --dir . calls birefnet xs132_pipeline/birefnet_calls.py
```
