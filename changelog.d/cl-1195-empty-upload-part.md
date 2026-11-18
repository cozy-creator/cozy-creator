Sending local code to a machine no longer fails with `a part is Content-Range: bytes a-b/length` when a part's bytes all landed
but its answer was lost: Calcifer sends only what is left of a part, and a machine takes an empty part as nothing to write.
