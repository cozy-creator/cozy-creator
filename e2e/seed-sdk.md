# Isolated Hub seed cohort

The candidate action exports the resolved Runtime/packages commits and the selected Runtime/TensorFS
inputs. Explicit wheel or release inputs win over source builds. The isolated wrapper stages those
exact bytes under the existing TLS ingress, then pins only the private copies of cohort packages
before `cozy package publish`. It verifies the returned locks still name those URLs, versions and
hashes. The publisher describes the completed lock's Runtime and TensorFS; it does not choose a
second rolling Runtime after locking.

A selected wheel must satisfy each package's declared requirement. Native wheels restrict a private
copy's lock to that runner's OS and architecture, intersecting any authored environment constraints.
Each OS has its own fixture. No package version range is raised or loosened. Old-floor and negative
fixtures keep their own selection, so their stable-floor refusals remain meaningful. An unreleased
dev floor uses an explicitly selected wheel; an admitted stable release still determines the oldest
floor that publication must support.

`seed-sdk.json` records each input, version, SHA256 and embedded Runtime commit. The command's
`cohort.json` adds the exact packages checkout, requested refs and verified publication lock hashes;
`cohort.txt` also reaches the GitHub step summary. `hub-requests.jsonl` includes the ingress's actual
SDK reads. The fixture's CA is passed with the existing `SSL_CERT_FILE` convention; certificate and
hostname checks remain enabled.

The focused `typed-calls` Linux CLI shard first seeds the four real package sources (Qwen, H3,
SDXL and BiRefNet), then runs the caller/child/leaf fixture through `cozy run`. This exercises both
publication's SDK reader and the nested root-lock reader. It rents no GPU and executes no model.
The ordinary default Linux matrix includes the cell.

For a reviewed feature cohort, use the existing workflow with the same Creator source for its
harness and candidate, plus exact Runtime and packages commits:

```sh
cohort=$(git rev-parse HEAD)
gh workflow run e2e.yaml --repo cozy-creator/cozy-creator \
  --ref codex/cl-994-seed-cohort-20261115 \
  -f repo=cozy-creator -f ref="$cohort" -f creator_ref="$cohort" -f stack_ref="$cohort" \
  -f runtime_ref=f584ef131cd8256499f9e1743637f6ef1e39794b \
  -f packages_ref=418452bcc44a13072d7c92850baa71398ccca375 \
  -f select=cli -f 'shards=["typed-calls"]'
```

Those Runtime/packages commits are the preserved prototype, not the 0.27 release cohort. The final
0.27 gate must substitute its reviewed Creator, Runtime 0.48 and package commits/artifacts together.
A draft source check or local ingress/lock/SDK-description check is not a passed publication/execution
gate. The focused dispatch above is prepared for coordinator review, not automatically dispatched.
