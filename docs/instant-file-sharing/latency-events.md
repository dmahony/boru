# Instant file-sharing latency events

Direct file offers use structured `tracing` events with the target
`boru::file_offer`. Each event has an `event` field whose value is one of the
stable names below. `offer_id` is the correlation key; `bytes` is included when
the milestone has a meaningful byte count.

| Event | Side | Meaning |
| --- | --- | --- |
| `file_selected` | sender | The user selected a regular file and metadata was read. |
| `offer_registered` | sender | The local-only offer registry entry was created. |
| `offer_broadcast` | sender | The small gossip offer was handed to the gossip sender. |
| `offer_received` | receiver | The receiver accepted the authenticated gossip offer. |
| `direct_download_requested` | receiver/sender | A receiver requested the direct stream (the sender also logs receipt of the request). |
| `first_byte_sent` | sender | The direct stream passed its header and was ready to write file bytes. |
| `first_byte_received` | receiver | The receiver read a non-empty direct stream chunk. |
| `blob_ingest_started` | sender | Background local blob-store preparation began after the offer broadcast. |
| `blob_ingest_completed` | sender | Background blob ingestion finished. |
| `blob_ticket_announced` | sender | The BlobTicket upgrade was broadcast after ingestion. |

## Latency contract

The sender's latency-critical sequence is:

```text
file_selected -> offer_registered -> offer_broadcast
```

No file read, hash, blob import, poster generation, or ticket creation occurs
between `file_selected` and `offer_broadcast`. Consequently the interval can be
compared across file sizes and should not scale with the selected file's byte
count. Blob ingestion and the ticket upgrade are deliberately logged after the
broadcast and run independently.

The constants live in `boru_core::diagnostics::event_names`; the sequence is
covered by the `direct_offer_latency_sequence_is_registration_before_broadcast`
unit test.
