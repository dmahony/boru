# Call encryption semantics

## Transport boundary

Boru call traffic is protected by the Iroh connection. The call protocol registers
`CALL_ALPN` (`/boru-call/1`) with Iroh and receives an authenticated
`iroh::endpoint::Connection` in `CallProtocol::accept`. Outgoing calls use
`Endpoint::connect` with the same ALPN. The control channel is then opened with
`Connection::open_bi` (`src/call/manager.rs`); its length prefix and postcard
encoding provide framing and schema validation only, not confidentiality.

Iroh establishes the QUIC connection with TLS 1.3. The peer's Iroh `PublicKey`
is the transport identity used for mutual authentication, and the connection
is encrypted end-to-end whether the QUIC path is direct or traverses an Iroh
relay. The relay forwards encrypted transport packets; it does not become a
call endpoint.

The `SecretKey` supplied when constructing `CallBuilder` is the Iroh identity
key associated with the endpoint. Call code does not derive application keys or
perform encryption itself. Authorization is an independent policy check (for
example, `denied_peers`) and must not be confused with transport encryption.

## Control and media

The currently implemented call path carries call setup, negotiation, state, and
teardown over the reliable bidirectional QUIC stream. There is no media codec
payload or media datagram implementation in `src/call` yet.

When media datagrams are implemented, they MUST be sent through the same Iroh
`Connection` (using its QUIC datagram API) or through another stream belonging
to that authenticated connection. QUIC protects datagrams and streams with the
same connection-level transport security. Media packet framing, codec payloads,
loss handling, and replay/ordering policy are separate protocol concerns; they
must not introduce a second hand-rolled encryption scheme.

An application-level ratchet, end-to-end media key protocol, or additional
payload encryption would be a separate cryptographic design project. It is not
part of the call protocol described here and must not be added implicitly as a
codec or packet-format change.

## Verification audit

The repository includes `scripts/audit-call-crypto.sh`. It scans Rust sources
under `src/call` for application-crypto markers and fails closed if one is
introduced:

```text
$ scripts/audit-call-crypto.sh
call crypto audit passed: no application-level crypto markers in src/call
```

The audit deliberately checks the call implementation only. Crypto used by
other Boru features (for example group encryption or pairing) is outside this
boundary and does not encrypt call transport traffic.
