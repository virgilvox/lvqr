# LVQR TURN deployment recipe

Operator runbook for adding a TURN server to an LVQR mesh
deployment so peers behind symmetric NAT can still relay media.

## Why TURN

The peer mesh uses WebRTC DataChannels. Two browsers on the same
LAN, or two browsers on networks with full-cone or restricted-cone
NATs, can typically establish a direct host-or-server-reflexive
candidate pair via the bundled Google STUN entry that
`MeshPeer` ships with by default. That covers most home and
office deployments.

Symmetric NAT (most carrier-grade NAT, some corporate firewalls)
allocates a different external port per destination, so
server-reflexive candidates from STUN are useless: each peer sees
a different external port for every other peer it tries to talk
to. The connection never establishes. TURN works around it: each
peer connects to the TURN relay, and the relay forwards media
between peers using a single allocation per session.

You only need this if your deployment includes peers on symmetric
NAT. STUN-only deployments can skip this directory entirely.

## What ships in this directory

- `coturn.conf` -- minimal coturn config covering the LVQR mesh
  case (UDP-only, long-term credentials, sane port range, sane
  quotas). Edit `realm`, `user`, and the port range before
  starting.
- This README.

## Install coturn

### Debian / Ubuntu

```sh
sudo apt-get install coturn
sudo cp coturn.conf /etc/coturn/coturn.conf
# Enable the service per your distro's conventions:
sudo systemctl enable --now coturn
```

### Alpine

```sh
sudo apk add coturn
sudo cp coturn.conf /etc/coturn/coturn.conf
sudo rc-service coturn start
```

### Docker

A community-maintained image ships at
`coturn/coturn`. Mount the config:

```sh
docker run -d --net=host \
  -v "$(pwd)/coturn.conf:/etc/coturn/turnserver.conf" \
  --name lvqr-turn coturn/coturn
```

`--net=host` is the simplest way to expose the relay port range;
the alternative is publishing every port from `min-port` to
`max-port` explicitly.

## Wire the running coturn into LVQR

Pass the same `urls`, `username`, and `credential` you set in
`coturn.conf` to `lvqr serve` via `--mesh-ice-servers`. The
flag accepts a JSON array of WebRTC `RTCIceServer` objects:

```sh
lvqr serve \
  --mesh-enabled \
  --mesh-ice-servers '[
    {"urls":["stun:stun.l.google.com:19302"]},
    {"urls":["turn:turn.example.com:3478"],
     "username":"lvqr-mesh",
     "credential":"<password>"}
  ]'
```

Or via the env var (cleaner for systemd / docker units):

```sh
LVQR_MESH_ICE_SERVERS='[{"urls":["turn:turn.example.com:3478"],"username":"lvqr-mesh","credential":"<password>"}]' \
  lvqr serve --mesh-enabled
```

The list flows down to every browser peer through the
`AssignParent` server-push message; clients automatically rebuild
their `RTCPeerConnection({ iceServers })` from this snapshot. No
client-side change is needed.

## Sanity check

The coturn distribution includes `turnutils_uclient` for
exercising the relay. From any machine that can reach the public
IP of the TURN server:

```sh
turnutils_uclient -u lvqr-mesh -w <password> turn.example.com
```

Successful output ends with `tot_send_bytes ~ tot_recv_bytes`
plus a non-zero per-channel bytes count. Failure points to
either firewall rules (the relay port range needs to be open) or
the credential not matching.

## Cost shape

TURN traffic flows through the TURN server's NIC. STUN is
stateless and effectively free; TURN is the opposite -- every
relayed peer pair costs `2 * stream_bitrate * concurrent_users`
bytes/sec on the TURN box. Plan capacity accordingly. Most
deployments add TURN as a fallback for the symmetric-NAT case
only and do not see the full mesh transit through it.

## Anti-scope (v1.1)

- **No TLS / DTLS configuration.** `coturn.conf` ships with
  `no-tls`; deployments that need TLS-wrapped TURN
  (`turns:` URL scheme) must add the cert + key paths and remove
  the `no-tls` line. Out of scope for this minimal recipe.
- **No short-lived credentials.** The static user-pass pair on
  disk is fine for a small operator-controlled deployment.
  Multi-tenant deployments should use coturn's REST API auth
  with HMAC-derived ephemeral creds; LVQR does not yet rotate
  the `--mesh-ice-servers` snapshot at runtime so the static
  path is the only supported shape today.
- **No autoscaling recipe.** A single coturn instance scales to
  thousands of concurrent relays on commodity hardware.
  Multi-region failover is a future operator concern.
