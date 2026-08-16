# Instant file sharing architecture

## V1 source-read decision (BORU-IFS-18)

Direct file offers and background blob ingest deliberately use independent
reads of the source file in V1. The sender announces `FileOffer` first, then
the direct-transfer handler reopens the registered local path when a peer
requests it (`src/file_offer_protocol.rs`), while the post-announcement ingest
task opens the same path separately before passing it to the blob store
(`examples/iced_chat/app/files.rs`). Each operation owns its file handle and
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