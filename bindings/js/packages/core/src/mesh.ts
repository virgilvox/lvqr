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
      };
    });
  }

  /** Disconnect from mesh. */
  close(): void {
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

  get childCount(): number {
    return this.children.size;
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
}
