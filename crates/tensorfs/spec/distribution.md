# cozytensors distribution/1

How a client reads and uploads cozytensors objects and a model's commits at an **endpoint**, so any server,
bucket or directory can serve them. Tensorhub implements it; `tfs conform endpoint` checks an implementation. Documents and
objects are as in [cozytensors.md](cozytensors.md). The key words are as in RFC 2119.

## 1. Resources

An endpoint is a base URL `B`. Its objects are blobs, and nothing lists them:

```text
B/blobs/<aa>/<bb>/<sha256>                an object: a tree, a cozytensors header, a chunk, an asset or a repo file
B/v1/models/<org>/<name>/resolve          a model's commit, resolved (§2)
B/v1/models/<org>/<name>/commits          a model's commits, recorded (§4)
```

A directory (a store) keeps the repo document (`cozytensors.repo/1`) at `B/refs/<org>/<name>.json` instead of the
two model routes, and a client resolves it itself (cozytensors.md §7). A static host or bucket serves objects only:
a commit is named by its id.

- `<org>/<name>` is the repo name with ASCII `A`–`Z` lowered; a segment other than letters, digits, `.`, `_` and
  `-` (or a device name such as `con`) is `%` and the sha256 of the lowered segment. `<aa>` and `<bb>` are the first
  two pairs of the hex digest. A store's directory has the same layout, so a directory, bucket or static host serves
  it as it is.
- URL construction percent-encodes each UTF-8 name segment separately after ASCII case folding; `/` remains
  the separator. A name containing a space, `%`, `?` or `#` must not alter the URL structure
  ([RFC 3986 §3.3](https://www.rfc-editor.org/rfc/rfc3986.html#section-3.3)).
- The org `local` is reserved for a machine's own repos and is never served.

## 2. Reading

```text
GET  B/v1/models/<org>/<name>/resolve?release=&lane=&gpus=&free_vram=&plain=   200 {release, lane, commit, header, reasons} | 404
GET  B/blobs/<aa>/<bb>/<hex>    [Range: bytes=a-b]          200 | 206 | 302/303/307 + Location | 404
HEAD B/blobs/<aa>/<bb>/<hex>                                200 + Content-Length | 404
```

- Resolve names one commit: of `release` (none: the newest), in `lane`, else what the hardware runs and fits:
  `gpus`, each GPU's compute capability (`8.9`) or Apple Metal family (`apple9`), none for the CPU, and
  `free_vram`, the free bytes on the smallest; `plain=true` takes plain lanes only (code that runs no encoded
  tensor). `reasons` say what was passed over.
- A client walks commit (tree) → its repo files and header → that header's chunks and assets. It MUST check every
  object's length and SHA-256 before it admits it.
- A commit no release names, and its objects, a server serves only to members of the model's organization.
- A server SHOULD honour one `bytes=a-b` range on a blob. A client MUST accept a `200` with the whole object instead.
- A blob GET MAY redirect. The client repeats the request at `Location` with its `Range`, follows at most 5 hops, and
  MAY reuse the target for restarts until the redirect's `Cache-Control: max-age` ends. Plain `http` is allowed only
  on loopback.

## 3. Uploading objects

For each object the client sends HEAD, then PUT when it is absent, then HEAD:

```text
HEAD B/blobs/<aa>/<bb>/<hex>            200 (present) | 404 [+ Location: where to PUT it]
PUT  <that Location, else the blob URL>
     Content-Length: <length>
     x-amz-checksum-sha256: <base64 of the digest>
     If-None-Match: *                   200 | 201 | 412 (present) | 400 digest_mismatch | 413 too_large
HEAD B/blobs/<aa>/<bb>/<hex>            200: the object is confirmed
```

- Order: chunks, assets and repo files first, then headers, then the tree. A tree present then implies its closure was
  sent.
- A server MUST store an object only when its bytes match the digest. `412` means it is already there, which is
  success: objects are content-addressed.
- The PUT always carries the three headers, so a presigned URL that signed them takes the same request. A server
  that redirects uploads verifies the object at the confirming HEAD. A confirming `404` with a `Location` the client
  has not sent to asks for the bytes there too (a second writer of stored bytes proving it holds them); one naming a
  place already sent to fails the upload.

## 4. Commits

```text
POST B/v1/models/<org>/<name>/commits {commit: {sha256, length}}      201 recorded | 200 held already | 409 | 400
```

- After its objects (§3), a client records the uploaded tree as a commit of the model; the first creates the model.
  A server checks the tree's lane and its one `.cozytensors` file, and that the caller may read its whole closure
  (`409 closure_incomplete`, up to 1,000 missing digests in `detail.missing`). It is private until a release names it.
- Releases and deletes are the server's own API (Tensorhub's `api/openapi.yaml`): a release names one commit per lane
  under a version higher than every earlier one, and is public for good.
- A server frees a commit's objects only after the model's owner deletes it, and then only those no other retained
  commit reaches.
- A directory records a commit in its store's repo document (cozytensors.md §7).

## 5. Authorization

- Reads of released commits and their objects are anonymous. Writes and private reads carry `Authorization`: `Bearer`,
  `DPoP` (RFC 9449, with `DPoP-Nonce` retries) or S3 SigV4.
- `401` carries `WWW-Authenticate`. Without read access an object or repo is `404`, never `403`.
- A client MUST NOT send credentials to another origin, after a redirect or a `Location`.

## 6. Errors

- An error body is `{"error": {"code", "message", "detail"?}}`.
- Codes: `not_found` 404, `unauthorized` 401, `denied` 403, `invalid` 400, `digest_mismatch` 400,
  `closure_incomplete` 409, `policy` 409, `too_large` 413 (`detail.limit`), `rate_limited` 429, `unavailable` 503,
  `unsupported` 405 (a read-only endpoint).
- A static server sends bare statuses; a client maps them by number.
- A client retries `429` and `503` at `Retry-After`, and transport failures while they make progress. It retries no
  other `4xx`, except one `401` after its credential provider refreshes.

## 7. Limits and versioning

- An object is at most 5 GiB (chunks are at most 64 MiB) and a document at most 16 MiB. A server MAY set lower
  limits and states them in `413` `detail.limit`.
- This is distribution/1. Documents carry their own format tags. New optional headers, codes and fields are minor and
  ignored by those who don't know them. A breaking change takes a new key root (`v2/…`). There is no handshake.

## 8. Endpoints without a server

| Endpoint | Reads | Writes |
|---|---|---|
| A directory, `file:///dir` | the files | objects by temp and rename, verified; commits in its repo document, under the store's record lock; an origin, so nothing expires or is evicted |
| A static host (nginx, Caddy, `python -m http.server`) | as is | none (`405`) |
| S3 or R2, `s3://bucket/prefix` | public, or SigV4 | SigV4: objects with `If-None-Match: *` and the checksum (the bucket verifies) |

A bucket has no closure check; the upload order keeps closures whole.
