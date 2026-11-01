# Fill and Arena retirement integration

Owner: /root/async_integration_review. Branch codex/xs138-p4-fill-retirement starts from
fetched origin/master551980610 and composes coordinator7a653de29 with checked async8b77294bb.
Native Arena retirement is being frozen independently by the P3 owner; no external P4
worktree is edited here. This is a draft integration, not GPU qualification or release.

The final native topology is one event-polling worker and one typed retirement queue per
device. The queue owns Arena finalization and checked Fill resources. A submitted Fill
retains every fixed producer stream, source/staging resource, producer Pin and exclusion
lease. Published data is available only to explicitly ordered consumers; Ready follows
checked producer completion. Pending resources remain owned without blocking later jobs.

The combined GPU fixture will request Arena retirement while a Fill and separately gated
ordered consumer retain real ownership. Another Arena must continue useful work. Producer
completion alone must not free consumer-backed bytes, and last-consumer completion must
permit exact final physical retirement without a context/device synchronize fallback.
The existing five standalone async modes remain intact. General TensorFS Source adoption,
public Source adapters, GPU execution and rental operation stay outside this follow-up.
