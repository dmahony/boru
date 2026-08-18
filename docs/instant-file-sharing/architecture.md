# Instant file sharing architecture

Status: design note for BORU-IFS-01

## Purpose

Instant file sharing separates two operations that are currently performed as one
send operation:

1. **File offer / chat announcement:** publish a small, authenticated offer that
   identifies the file by an `offer_id`, safe basename, byte size, and sender
   identity. This is the user-visible operation. A receiver may show the
   attachment and request bytes as soon as the offer arrives.
2. **Blob ingest / cache preparation:** independently read the source, hash it,
   and import it into Boru's Iroh blob store. When this completes, the sender
   may publish a correlated upgrade containing the cached, content-addressed
   transfer information.

An offer is therefore not proof that the file has been ingested into the local
blob store. Conversely, local blob ingestion is not an outbound network
"upload". The offer and all later upgrades are correlated by `offer_id` (and,
where required, the authenticated sender public key), never by filename.

V1's new direct-transfer path is for direct conversations. Groups and public
rooms continue using the existing BlobTicket/FileShare path until direct
streaming is proven stable. Sender filesystem paths are local-only and must
never appear in gossip or transfer requests.

## Current send path and planned split

At the time of this note, `ExecuteFileSend` in
`src/bin/boru/app/files.rs` performs the following work:

- It validates the negotiated file capability, obtains the local source path,
  and creates the local transfer card immediately.
- Its asynchronous send task checks the source-path mapping in storage and
  confirms that the mapped blob still exists.
- If no usable cached blob is found, it opens and streams the entire source into
  `blob_store.blobs().add_stream(...)`, walking the import stream to report
  progress and obtain the content hash and blob format.
- It then creates a serialized BlobTicket and broadcasts the legacy
  `Message::FileShare` containing the ticket and size.
- Video poster generation and poster-blob storage happen after that first
  broadcast; a follow-up FileShare can add the poster hash.
- Finally, sender bookkeeping records the source-path-to-blob mapping, and the
  existing transfer-progress lifecycle reports completion.

The first three bullets currently make a new file's chat announcement wait for
full blob ingest. Later BORU-IFS tasks will split the path at this boundary:

```text
file selection
    -> local offer registration (safe basename, size, offer_id)
    -> FileOffer broadcast
    -> receiver Ready attachment and direct-transfer request

                         in parallel
source path
    -> background blob ingest/cache preparation
    -> BlobTicket creation
    -> offer_id-correlated cached-transfer upgrade/fallback
```

The direct-transfer implementation must reserve and validate a safe destination,
check the authenticated sender and offer state, verify the expected byte count
and completion hash, and feed the existing `TransferProgress` events. A failed
background ingest must not invalidate a still-valid direct offer. If direct
transfer is unavailable or fails and a cached ticket exists, the existing
BlobTicket route remains the fallback.

## Existing blob system retained

This change is an architectural separation, not a blob-system replacement. The
following remain supported and are deliberately preserved:

- the Iroh blob store and its content-addressed blobs;
- BlobTicket creation, parsing, and ticket-based downloads;
- source hashing and blob verification;
- the source-path-to-known-blob re-share/cache fast path;
- video poster generation and poster blobs;
- download byte-count/integrity verification;
- the existing progress, completion, and download-card UX; and
- compatibility with legacy FILES-v1 peers through the existing FileShare
  mechanism.

### Re-share fast path

The current fast path reads the stored blob hash for the exact source path and
then calls the blob store's existence check. Only when both the mapping parses as
an Iroh hash and that blob is still present does it skip re-ingestion and build a
BlobTicket from the known hash. A stale or missing blob mapping falls back to
normal ingest. Later offer work must retain this exact source-path plus
blob-existence validation; it must not treat a stale mapping as a valid cache.

## Non-goals and invariants

- Do not delete or refactor away blob ingest, BlobTickets, hashing, poster
  generation, or download verification.
- Do not change the image pipeline (`ExecuteImageSend`); its image-specific
  pre-announcement behavior remains unchanged.
- Do not put an absolute source path in a chat message, offer, history record, or
  network request.
- Do not use a filename as an offer identity; two offers with the same basename
  are independent transfers.
- Do not read, hash, or import the complete file before the FileOffer is
  announced.

This note documents the intended boundary for the subsequent implementation
tasks. It does not change production behavior by itself.

---

## V1 source-read decision (BORU-IFS-18)

Direct file offers and background blob ingest deliberately use independent
reads of the source file in V1. The sender announces `FileOffer` first, then
the direct-transfer handler reopens the registered local path when a peer
requests it (`src/file_offer_protocol.rs`), while the post-announcement ingest
task opens the same path separately before passing it to the blob store
(`src/bin/boru/app/files.rs`). Each operation owns its file handle and
cursor; neither path shares a seek position or takes an exclusive source-file
lock. This allows a direct download to proceed even if blob ingest is slow or
fails.

The independent reads are an intentional correctness and lifecycle choice, not
an oversight. V1 does not require a zero-copy or single-read fan-out pipeline.
Such a pipeline could later feed the direct network stream and blob-ingest/cache
writer from one bounded reader, but it would add cancellation, backpressure,
and failure-isolation complexity. That optimization is deferred until direct
transfer behavior is proven stable.

The direct path remains independently available after announcement. Blob
ingest is an asynchronous cache/preparation operation and only publishes the
`FileOfferReady` BlobTicket after it completes. No local filesystem path is
included in gossip or in a network request; paths remain in the sender's
process-local offer registry.

The concurrency smoke test in
`src/file_offer_protocol.rs` opens the source independently for a direct read
and a blob-store stream, runs both operations concurrently, and verifies the
complete direct bytes and resulting blob hash.
