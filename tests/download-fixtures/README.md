# Desktop download fixtures

These files are deterministic inputs for the desktop download test. The expected SHA-256 digest, byte size, MIME type, and setup workflow for every case are recorded in `manifest.json`.

Regenerate the files with:

```text
python3 tests/download-fixtures/generate.py
```

Required cases:

- `zero-byte.txt`: zero-byte edge case. It is a direct-download fixture; the application import validator intentionally rejects empty files.
- `small-message.txt`: small text file.
- `large-deterministic.bin`: 8 MiB generated binary file. Each byte at offset `n` is `n % 251`.
- `imported-document.json`: must be added through the File Library Import workflow, using this file as the source. Do not model it only by copying a file into managed storage.
- `referenced-record.csv`: must be added through the File Library Reference/Offer workflow, retaining this fixture as the source path. Do not model it only as a local copy.
- `duplicate-a.txt` and `duplicate-b.txt`: different filenames with identical bytes, for deduplication and same-content download checks.

The application uses BLAKE3 for content addressing. The manifest records SHA-256 because it is the externally auditable fixture checksum; the download test should also verify the application-reported content hash where applicable.
