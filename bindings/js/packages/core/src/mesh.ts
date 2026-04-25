/**
 * WebRTC mesh peer client for LVQR P2P media relay.
 *
 * When connected to the signal server, peers receive tree assignments
 * (AssignParent). Non-root peers establish WebRTC DataChannel connections
 * to their assigned parent and relay media frames to their children.
 *
 * Data flow:
 *   Server -> Root peers (via MoQ/WebTransport or WS)
 *   Root peers -> Child peers (via WebRTC DataChannel)
 *   Relay peers -> Their children (via WebRTC DataChannel)
 */

export interface MeshConfig {
  /** Signal server WebSocket URL (e.g. "ws://localhost:8080/signal"). */
  signalUrl: string;
  /** This peer's unique ID. */
  peerId: string;
  /** Track/broadcast to subscribe to. */
  track: string;
  /** STUN servers for ICE. */
  iceServers?: RTCIceServer[];
  /** Called when a media frame is received from the parent. */
  onFrame?: (data: Uint8Array) => void;
  /**
   * Called on the parent side when a child's DataChannel transitions
   * to `open`. Fires once per child per connection. Integrators that
   * want to push a one-shot payload the moment a child is ready (e.g.
   * an init segment buffered from the server) can use this callback
   * instead of polling `childCount`. The `childId` is the peer_id the
   * child registered with; `dc` is the DataChannel the fanout path
   * will use for `pushFrame`.
   */
  onChildOpen?: (childId: string, dc: RTCDataChannel) => void;
}

interface PeerConnection {
  pc: RTCPeerConnection;
  dc: RTCDataChannel | null;
  peerId: string;
}

/**
 * Mesh peer that participates in the P2P relay tree.
 *
 * @example
 * ```ts
 * const mesh = new MeshPeer({
 *   signalUrl: 'ws://localhost:8080/signal',
 *   peerId: crypto.randomUUID(),
 *   track: 'live/my-stream',
 *   onFrame: (data) => sourceBuffer.appendBuffer(data),
 * });
 * await mesh.connect();
 * ```
 */
export class MeshPeer {
  private config: MeshConfig;
  private signal: WebSocket | null = null;
  private role: string = 'unknown';
  private parentId: string | null = null;
  private parentConn: PeerConnection | null = null;
  private children = new Map<string, PeerConnection>();
  private iceConfig: RTCConfiguration;
  /** Cumulative count of fragments forwarded to DataChannel children.
   *  Incremented inside `forwardToChildren` per successful `dc.send()`.
   *  Reported to the server via `ForwardReport` on a 1 s interval so
   *  `GET /api/v1/mesh` can surface actual-vs-intended offload.
   *  Session 141. */
  private forwardedFrames: number = 0;
  private lastReportedFrames: number = 0;
  private reportInterval: ReturnType<typeof setInterval> | null = null;

  constructor(config: MeshConfig) {
    this.config = config;
    this.iceConfig = {
      iceServers: config.iceServers ?? [{ urls: 'stun:stun.l.google.com:19302' }],
    };
  }

  /** Connect to the signal server and register. */
  async connect(): Promise<void> {
    return new Promise((resolve, reject) => {
      this.signal = new WebSocket(this.config.signalUrl);

      this.signal.onopen = () => {
        this.signal!.send(JSON.stringify({
          type: 'Register',
          peer_id: this.config.peerId,
          track: this.config.track,
        }));
        this.startForwardReportLoop();
        resolve();
      };

      this.signal.onerror = () => reject(new Error('signal connection failed'));

      this.signal.onmessage = (event) => {
        try {
          const msg = JSON.parse(event.data);
          this.handleSignalMessage(msg);
        } catch {
          // ignore invalid JSON
        }
      };

      this.signal.onclose = () => {
        this.signal = null;
        this.stopForwardReportLoop();
      };
    });
  }

  /** Disconnect from mesh. */
  close(): void {
    this.stopForwardReportLoop();
    this.parentConn?.pc.close();
    this.parentConn = null;
    for (const child of this.children.values()) {
      child.pc.close();
    }
    this.children.clear();
    this.signal?.close();
    this.signal = null;
  }

  get peerRole(): string {
    return this.role;
  }

  /**
   * The peer_id of the assigned parent, or `null` for Root peers
   * that connect directly to the origin server. Driven by the
   * server-pushed `AssignParent` message; reads `null` until the
   * first assignment lands. Session 142 -- exposed as a getter so
   * deterministic chain-formation waits in tests no longer have
   * to fish in private state.
   */
  get parentPeerId(): string | null {
    return this.parentId;
  }

  get childCount(): number {
    return this.children.size;
  }

  /**
   * Cumulative count of fragments this peer has forwarded to its
   * DataChannel children. Matches the `forwarded_frames` value the
   * peer reports to the server via the `/signal` `ForwardReport`
   * message (see `crates/lvqr-signal`). Reset on close(); preserved
   * across WS reconnects by the client-side counter (the server
   * tolerates resets via replace-rather-than-accumulate semantics).
   * Session 141.
   */
  get forwardedFrameCount(): number {
    return this.forwardedFrames;
  }

  /**
   * Inject a media frame received from upstream (server) into the local
   * mesh fanout. Used by root peers: a root has no parent DataChannel to
   * drive `forwardToChildren`, so the integrator drains media from the
   * server (via MoQ/WebTransport or WS) and calls `pushFrame` on every
   * chunk to forward it to subscribed children.
   *
   * Non-root peers normally do not need this method; their fanout is
   * driven from the parent-side `dc.onmessage` path. Calling `pushFrame`
   * on a non-root peer is legal (it forwards to children) but bypasses
   * the `onFrame` local-consumer callback.
   */
  pushFrame(data: Uint8Array): void {
    this.forwardToChildren(data);
  }

  private handleSignalMessage(msg: Record<string, unknown>): void {
    switch (msg.type) {
      case 'AssignParent':
        this.handleAssignment(msg);
        break;
      case 'Offer':
        this.handleOffer(msg as { from: string; sdp: string });
        break;
      case 'Answer':
        this.handleAnswer(msg as { from: string; sdp: string });
        break;
      case 'IceCandidate':
        this.handleIceCandidate(msg as { from: string; candidate: string });
        break;
      case 'PeerLeft':
        this.handlePeerLeft(msg as { peer_id: string });
        break;
    }
  }

  private handleAssignment(msg: Record<string, unknown>): void {
    this.role = msg.role as string;
    this.parentId = (msg.parent_id as string | null) ?? null;

    // Session 143: server-driven ICE config. When the operator
    // booted lvqr with `--mesh-ice-servers '[...]'`, every
    // AssignParent carries the configured list. A non-empty list
    // is authoritative -- rebuild iceConfig from the snapshot so
    // future RTCPeerConnections (parent-side and child-side) pick
    // up the operator's STUN/TURN entries automatically. Empty
    // list (operator did not configure the flag) leaves the
    // constructor-provided iceConfig untouched, preserving
    // backward compat for integrators who pass their own list.
    const serverIceServers = msg.ice_servers as RTCIceServer[] | undefined;
    if (Array.isArray(serverIceServers) && serverIceServers.length > 0) {
      this.iceConfig = { iceServers: serverIceServers };
    }

    if (this.parentId) {
      // Non-root: connect to parent via WebRTC
      this.connectToParent(this.parentId);
    }
  }

  /** Initiate WebRTC connection to parent peer. */
  private async connectToParent(parentId: string): Promise<void> {
    const pc = new RTCPeerConnection(this.iceConfig);
    const dc = pc.createDataChannel('media', { ordered: true });

    dc.binaryType = 'arraybuffer';
    dc.onmessage = (event) => {
      const data = new Uint8Array(event.data);
      // Deliver to local consumer
      this.config.onFrame?.(data);
      // Relay to children
      this.forwardToChildren(data);
    };

    pc.onicecandidate = (event) => {
      if (event.candidate) {
        this.sendSignal({
          type: 'IceCandidate',
          from: this.config.peerId,
          to: parentId,
          candidate: event.candidate.candidate,
        });
      }
    };

    this.parentConn = { pc, dc, peerId: parentId };

    const offer = await pc.createOffer();
    await pc.setLocalDescription(offer);

    this.sendSignal({
      type: 'Offer',
      from: this.config.peerId,
      to: parentId,
      sdp: offer.sdp!,
    });
  }

  /** Handle an SDP offer from a child peer wanting to connect. */
  private async handleOffer(msg: { from: string; sdp: string }): Promise<void> {
    const pc = new RTCPeerConnection(this.iceConfig);

    pc.ondatachannel = (event) => {
      const dc = event.channel;
      dc.binaryType = 'arraybuffer';
      // Store the child connection with its DataChannel
      const existing = this.children.get(msg.from);
      if (existing) {
        existing.dc = dc;
      }
      // Fire `onChildOpen` once the channel transitions to open so
      // integrators can push a one-shot payload (e.g. init segment)
      // the moment the channel is usable. `dc.readyState` is
      // typically "connecting" here and flips to "open" after SCTP
      // handshake completes.
      if (dc.readyState === 'open') {
        try {
          this.config.onChildOpen?.(msg.from, dc);
        } catch {
          // swallow: integrator callback errors should not tear down
          // the parent-side state machine.
        }
      } else {
        dc.addEventListener('open', () => {
          try {
            this.config.onChildOpen?.(msg.from, dc);
          } catch {
            // swallow
          }
        });
      }
    };

    pc.onicecandidate = (event) => {
      if (event.candidate) {
        this.sendSignal({
          type: 'IceCandidate',
          from: this.config.peerId,
          to: msg.from,
          candidate: event.candidate.candidate,
        });
      }
    };

    this.children.set(msg.from, { pc, dc: null, peerId: msg.from });

    await pc.setRemoteDescription({ type: 'offer', sdp: msg.sdp });
    const answer = await pc.createAnswer();
    await pc.setLocalDescription(answer);

    this.sendSignal({
      type: 'Answer',
      from: this.config.peerId,
      to: msg.from,
      sdp: answer.sdp!,
    });
  }

  /** Handle an SDP answer from the parent peer. */
  private async handleAnswer(msg: { from: string; sdp: string }): Promise<void> {
    if (this.parentConn && this.parentConn.peerId === msg.from) {
      await this.parentConn.pc.setRemoteDescription({ type: 'answer', sdp: msg.sdp });
    }
  }

  /** Handle an ICE candidate from a peer. */
  private async handleIceCandidate(msg: { from: string; candidate: string }): Promise<void> {
    const conn = this.parentConn?.peerId === msg.from
      ? this.parentConn
      : this.children.get(msg.from);

    if (conn) {
      await conn.pc.addIceCandidate({ candidate: msg.candidate, sdpMid: '0', sdpMLineIndex: 0 });
    }
  }

  /** Handle a peer leaving the mesh. */
  private handlePeerLeft(msg: { peer_id: string }): void {
    const child = this.children.get(msg.peer_id);
    if (child) {
      child.pc.close();
      this.children.delete(msg.peer_id);
    }
  }

  /** Forward frame data to all connected children. */
  private forwardToChildren(data: Uint8Array): void {
    for (const child of this.children.values()) {
      if (child.dc && child.dc.readyState === 'open') {
        try {
          child.dc.send(data as unknown as ArrayBuffer);
          // Session 141: count one send per child per frame. Operators
          // reading `/api/v1/mesh` see this as the peer's actual
          // forwarded_frames count; the topology planner's
          // intended_children is a separate field.
          this.forwardedFrames += 1;
        } catch {
          // DataChannel may be closing
        }
      }
    }
  }

  /** Send a signal message via WebSocket. */
  private sendSignal(msg: Record<string, unknown>): void {
    if (this.signal?.readyState === WebSocket.OPEN) {
      this.signal.send(JSON.stringify(msg));
    }
  }

  /** Start the 1-second forward-report emitter. Session 141. */
  private startForwardReportLoop(): void {
    // Guard against a duplicate loop on reconnect.
    this.stopForwardReportLoop();
    this.reportInterval = setInterval(() => {
      // Skip-on-unchanged: idle peers and leaves that never forward
      // stay silent on the signaling channel.
      if (this.forwardedFrames === this.lastReportedFrames) {
        return;
      }
      this.sendSignal({ type: 'ForwardReport', forwarded_frames: this.forwardedFrames });
      this.lastReportedFrames = this.forwardedFrames;
    }, 1000);
  }

  private stopForwardReportLoop(): void {
    if (this.reportInterval !== null) {
      clearInterval(this.reportInterval);
      this.reportInterval = null;
    }
  }
}
