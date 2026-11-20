# Suite `capability` (the coordinator 11-20, of 44636417703: "A private LoRA or model in a local render over 15 minutes failing
# mid-run is a user-facing loss in the release"): a project's run on this computer's machine still reads the Hub under its run
# capability after the token it first redeemed has expired (the Hub's 15 min). It needs no rental: skus names none, and
# release-gate.yml's `hub-fixture` job runs run.sh on a runner against CI's throwaway Hub.
