/**
 * MoQ-Lite wire protocol implementation for WebTransport.
 *
 * Implements the subset of moq-lite needed for subscribing to tracks:
 * - VarInt encoding (QUIC-style)
 * - ANNOUNCE_PLEASE / ANNOUNCE messages (track discovery)
 * - SUBSCRIBE / SUBSCRIBE_OK messages (track subscription)
 * - GROUP / FRAME reading (data reception)
 *
 * Targets moq-lite-01 (simplest wire format).
 */

// --- VarInt codec (QUIC RFC 9000 Section 16) ---

export function encodeVarInt(value: number): Uint8Array {
  if (value < 0x40) {
    return new Uint8Array([value]);
  } else if (value < 0x4000) {
    return new Uint8Array([(value >> 8) | 0x40, value & 0xFF]);
  } else if (value < 0x40000000) {
    const buf = new Uint8Array(4);
    buf[0] = (value >> 24) | 0x80;
    buf[1] = (value >> 16) & 0xFF;
    buf[2] = (value >> 8) & 0xFF;
    buf[3] = value & 0xFF;
    return buf;
  } else {
    const buf = new Uint8Array(8);
    buf[0] = 0xC0; // top 2 bits = 11
    // JS safe integer range is 2^53, this handles up to 2^62
    buf[1] = 0;
    buf[2] = (value / 0x1000000000000) & 0xFF;
    buf[3] = (value / 0x10000000000) & 0xFF;
    buf[4] = (value >> 24) & 0xFF;
    buf[5] = (value >> 16) & 0xFF;
    buf[6] = (value >> 8) & 0xFF;
    buf[7] = value & 0xFF;
    return buf;
  }
}

export function decodeVarInt(buf: Uint8Array, offset: number): [number, number] {
  const first = buf[offset];
  const prefix = first >> 6;

  if (prefix === 0) {
    return [first & 0x3F, offset + 1];
  } else if (prefix === 1) {
    const value = ((first & 0x3F) << 8) | buf[offset + 1];
    return [value, offset + 2];
  } else if (prefix === 2) {
    const value =
      ((first & 0x3F) << 24) |
      (buf[offset + 1] << 16) |
      (buf[offset + 2] << 8) |
      buf[offset + 3];
    return [value, offset + 4];
  } else {
    // 8-byte varint (we only handle values up to 2^53)
    let value = 0;
    for (let i = 0; i < 8; i++) {
      value = value * 256 + (i === 0 ? buf[offset] & 0x3F : buf[offset + i]);
    }
    return [value, offset + 8];
  }
}

// --- Binary helpers ---

function encodeString(s: string): Uint8Array {
  const encoded = new TextEncoder().encode(s);
  const lenBytes = encodeVarInt(encoded.length);
  const result = new Uint8Array(lenBytes.length + encoded.length);
  result.set(lenBytes);
  result.set(encoded, lenBytes.length);
  return result;
}

function decodeString(buf: Uint8Array, offset: number): [string, number] {
  const [len, newOffset] = decodeVarInt(buf, offset);
  const str = new TextDecoder().decode(buf.slice(newOffset, newOffset + len));
  return [str, newOffset + len];
}

/** Encode a MoQ Path (array of segments). */
function encodePath(segments: string[]): Uint8Array {
  const parts: Uint8Array[] = [encodeVarInt(segments.length)];
  for (const seg of segments) {
    parts.push(encodeString(seg));
  }
  let totalLen = 0;
  for (const p of parts) totalLen += p.length;
  const result = new Uint8Array(totalLen);
  let off = 0;
  for (const p of parts) {
    result.set(p, off);
    off += p.length;
  }
  return result;
}

function decodePath(buf: Uint8Array, offset: number): [string[], number] {
  const [count, off1] = decodeVarInt(buf, offset);
  const segments: string[] = [];
  let off = off1;
  for (let i = 0; i < count; i++) {
    const [seg, newOff] = decodeString(buf, off);
    segments.push(seg);
    off = newOff;
  }
  return [segments, off];
}

/** Concatenate multiple Uint8Arrays. */
function concat(...arrays: ArrayLike<number>[]): Uint8Array {
  let total = 0;
  for (const a of arrays) total += a.length;
  const result = new Uint8Array(total);
  let off = 0;
  for (const a of arrays) {
    result.set(a, off);
    off += a.length;
  }
  return result;
}

/** Size-prefix a message body (varint length + body). */
function sizePrefix(body: Uint8Array): Uint8Array {
  return concat(encodeVarInt(body.length), body);
}

// --- Control stream types ---

const CONTROL_TYPE_ANNOUNCE = 1;
const CONTROL_TYPE_SUBSCRIBE = 2;

// --- Announce stream ---

/** Encode an AnnouncePlease message (request all announcements). */
export function encodeAnnouncePlease(prefix: string): Uint8Array {
  // Path with prefix segments. Empty string = 0 segments = request everything.
  // Must match Rust's Path::from("") which produces Vec::new() (0 segments).
  const segments = prefix ? prefix.split('/') : [];
  const pathBytes = encodePath(segments);
  return sizePrefix(pathBytes);
}

export interface AnnounceMessage {
  active: boolean;
  path: string[];
}

/** Decode an Announce message from the wire. Returns null if stream ended. */
export function decodeAnnounce(buf: Uint8Array, offset: number): [AnnounceMessage | null, number] {
  if (offset >= buf.length) return [null, offset];

  // Size prefix
  const [size, off1] = decodeVarInt(buf, offset);
  if (off1 + size > buf.length) return [null, offset]; // incomplete

  // Status byte (0 = ended, 1 = active)
  const status = buf[off1];
  const active = status === 1;

  // Path
  const [path, off2] = decodePath(buf, off1 + 1);

  return [{ active, path }, off1 + size];
}

// --- AnnounceInit (Lite01/02: list of initial broadcasts) ---

export interface AnnounceInit {
  suffixes: string[][];
}

export function decodeAnnounceInit(buf: Uint8Array, offset: number): [AnnounceInit, number] {
  // Size prefix
  const [size, off1] = decodeVarInt(buf, offset);
  const end = off1 + size;

  const [count, off2] = decodeVarInt(buf, off1);
  const suffixes: string[][] = [];
  let off = off2;
  for (let i = 0; i < count; i++) {
    const [path, newOff] = decodePath(buf, off);
    suffixes.push(path);
    off = newOff;
  }

  return [{ suffixes }, end];
}

// --- Subscribe stream ---

/** Encode a Subscribe message (moq-lite-01 format). */
export function encodeSubscribe(
  id: number,
  broadcastPath: string[],
  trackName: string,
  priority: number,
): Uint8Array {
  const body = concat(
    encodeVarInt(id),
    encodePath(broadcastPath),
    encodeString(trackName),
    new Uint8Array([priority & 0xFF]),  // priority is u8, not varint
  );
  return sizePrefix(body);
}

export interface SubscribeOk {
  priority: number;
}

/** Decode a SubscribeOk response (moq-lite-01 format). */
export function decodeSubscribeOk(buf: Uint8Array, offset: number): [SubscribeOk, number] {
  const [size, off1] = decodeVarInt(buf, offset);
  const [priority, off2] = decodeVarInt(buf, off1);
  return [{ priority }, off1 + size];
}

// --- Data stream (unidirectional) ---

const DATA_TYPE_GROUP = 0;

export interface GroupHeader {
  subscribeId: number;
  sequence: number;
}

/** Decode a Group header from the start of a unidirectional stream. */
export function decodeGroupHeader(buf: Uint8Array, offset: number): [GroupHeader, number] {
  // Size prefix
  const [size, off1] = decodeVarInt(buf, offset);
  const [subscribeId, off2] = decodeVarInt(buf, off1);
  const [sequence, off3] = decodeVarInt(buf, off2);
  return [{ subscribeId, sequence }, off1 + size];
}

// --- SETUP handshake ---
//
// When a browser connects via WebTransport (no ALPN negotiation),
// the server uses IETF Draft14 encoding for the SETUP exchange:
// - Type: u8 (CLIENT_SETUP=0x20, SERVER_SETUP=0x21)
// - Size: u16 big-endian (NOT varint)
// - Body: varint fields
//
// After SETUP, the session switches to the negotiated version (Lite01).

const CLIENT_SETUP = 0x20;
const SERVER_SETUP = 0x21;
const VERSION_LITE01 = 0xff0dad01;

/** Encode u16 as big-endian bytes. */
function encodeU16(value: number): Uint8Array {
  return new Uint8Array([(value >> 8) & 0xFF, value & 0xFF]);
}

/** Decode u16 from big-endian bytes. */
function decodeU16(buf: Uint8Array, offset: number): [number, number] {
  const value = (buf[offset] << 8) | buf[offset + 1];
  return [value, offset + 2];
}

/** Encode CLIENT_SETUP message (IETF Draft14 wire format). */
function encodeClientSetup(): Uint8Array {
  // Body: varint version_count + varint version_code + parameters
  // Parameters: IETF format with MaxRequestId and Implementation
  // For simplicity, send empty parameters (server doesn't require them)
  const body = concat(
    encodeVarInt(1),               // 1 supported version
    encodeVarInt(VERSION_LITE01),  // moq-lite-01 (0xff0dad01)
    // empty parameters
  );
  // CLIENT_SETUP: [u8 type][u16 BE size][body]
  return concat(
    new Uint8Array([CLIENT_SETUP]),
    encodeU16(body.length),
    body,
  );
}

/** Read SERVER_SETUP response (IETF Draft14 wire format). */
function decodeServerSetup(buf: Uint8Array, offset: number): [number, number] {
  const type_ = buf[offset];
  if (type_ !== SERVER_SETUP) {
    throw new Error(`expected SERVER_SETUP (0x21), got 0x${type_.toString(16)}`);
  }
  offset += 1;

  // u16 big-endian size
  const [size, off1] = decodeU16(buf, offset);
  const end = off1 + size;

  // varint version code
  const [version, _off2] = decodeVarInt(buf, off1);

  return [version, end];
}

// --- High-level MoQ subscriber ---

export interface MoqTrack {
  broadcast: string;
  name: string;
}

/**
 * MoQ subscriber that connects to a relay via WebTransport.
 *
 * Performs SETUP handshake (moq-lite-01), discovers broadcasts via ANNOUNCE,
 * subscribes to tracks, and emits frame data via callbacks.
 */
export class MoqSubscriber {
  private wt: WebTransport;
  private subscriptions = new Map<number, { onFrame: (data: Uint8Array) => void }>();
  private nextId = 0;
  private running = false;
  private setupDone = false;

  constructor(wt: WebTransport) {
    this.wt = wt;
  }

  /** Perform the MoQ SETUP handshake. Must be called before subscribe/discover. */
  async setup(): Promise<void> {
    if (this.setupDone) return;

    const bidi = await this.wt.createBidirectionalStream();
    const writer = bidi.writable.getWriter();
    const reader = bidi.readable.getReader();

    // Send CLIENT_SETUP
    await writer.write(encodeClientSetup());

    // Read SERVER_SETUP
    const { value } = await reader.read();
    if (!value) throw new Error('setup stream closed before server response');

    const buf = Uint8Array.from(value);
    const [version] = decodeServerSetup(buf, 0);

    if (version !== VERSION_LITE01) {
      throw new Error(`server chose unsupported version 0x${version.toString(16)}, expected moq-lite-01`);
    }

    // Setup stream stays open for session lifetime (moq-lite uses it for SessionInfo)
    // Release the locks but keep the stream alive
    reader.releaseLock();
    writer.releaseLock();

    this.setupDone = true;
  }

  /** Discover broadcasts by sending ANNOUNCE_PLEASE and reading announcements. */
  async discoverBroadcasts(prefix = ''): Promise<string[]> {
    await this.setup();

    const bidi = await this.wt.createBidirectionalStream();
    const writer = bidi.writable.getWriter();
    const reader = bidi.readable.getReader();

    // Write: ControlType::Announce
    await writer.write(encodeVarInt(CONTROL_TYPE_ANNOUNCE));

    // Write: AnnouncePlease
    await writer.write(encodeAnnouncePlease(prefix));

    // Read AnnounceInit (Lite01: size-prefixed list of paths)
    const { value } = await reader.read();
    if (!value) return [];

    const [init] = decodeAnnounceInit(Uint8Array.from(value), 0);
    const broadcasts = init.suffixes.map((segs) => segs.join('/'));

    reader.releaseLock();
    writer.releaseLock();

    return broadcasts;
  }

  /** Subscribe to a track and start receiving frames. */
  async subscribe(
    broadcastPath: string[],
    trackName: string,
    onFrame: (data: Uint8Array) => void,
  ): Promise<number> {
    await this.setup();

    const id = this.nextId++;
    this.subscriptions.set(id, { onFrame });

    const bidi = await this.wt.createBidirectionalStream();
    const writer = bidi.writable.getWriter();
    const reader = bidi.readable.getReader();

    // Write: ControlType::Subscribe
    await writer.write(encodeVarInt(CONTROL_TYPE_SUBSCRIBE));

    // Write: Subscribe message
    await writer.write(encodeSubscribe(id, broadcastPath, trackName, 0));

    // Read: SubscribeOk
    const { value } = await reader.read();
    if (!value) throw new Error('subscribe stream closed before ok');

    // SubscribeOk received -- subscription is active
    reader.releaseLock();
    writer.releaseLock();

    // Start reading data streams if not already running
    if (!this.running) {
      this.running = true;
      this.readDataStreams();
    }

    return id;
  }

  /** Background loop: accept incoming unidirectional streams (data). */
  private async readDataStreams(): Promise<void> {
    const reader = this.wt.incomingUnidirectionalStreams.getReader();

    try {
      while (true) {
        const { value: stream, done } = await reader.read();
        if (done || !stream) break;
        this.handleDataStream(stream);
      }
    } catch {
      // Transport closed
    }
  }

  /** Handle a single incoming data stream (one MoQ group). */
  private async handleDataStream(stream: ReadableStream<Uint8Array>): Promise<void> {
    const reader = stream.getReader();
    let headerParsed = false;
    let subscribeId = -1;
    let buffer: Uint8Array<ArrayBufferLike> = new Uint8Array(0);

    try {
      while (true) {
        const result = await reader.read();
        if (result.done || !result.value) break;

        // Accumulate bytes (TS 5.9 Uint8Array generic workaround)
        // eslint-disable-next-line @typescript-eslint/no-explicit-any
        buffer = concat(buffer, result.value as any);

        if (!headerParsed) {
          // Need: DataType (varint) + Group header (size-prefixed: subscribeId + sequence)
          if (buffer.length < 3) continue;

          let offset = 0;
          // DataType::Group = 0
          const [dataType, off1] = decodeVarInt(buffer, offset);
          if (dataType !== DATA_TYPE_GROUP) return;
          offset = off1;

          // Group header
          const [group, off2] = decodeGroupHeader(buffer, offset);
          subscribeId = group.subscribeId;
          offset = off2;

          // Remove parsed header from buffer
          buffer = buffer.slice(offset);
          headerParsed = true;
        }

        // Read frames: each frame is varint_size + payload
        let offset = 0;
        while (offset < buffer.length) {
          const remaining = buffer.length - offset;
          if (remaining < 1) break;

          // Try to decode frame size
          const [frameSize, sizeEnd] = decodeVarInt(buffer, offset);
          const frameEnd = sizeEnd + frameSize;

          if (frameEnd > buffer.length) break; // incomplete frame

          // Extract frame data
          const frameData = buffer.slice(sizeEnd, frameEnd);

          // Deliver to subscriber
          const sub = this.subscriptions.get(subscribeId);
          if (sub) {
            sub.onFrame(frameData);
          }

          offset = frameEnd;
        }

        // Keep unprocessed remainder
        if (offset > 0) {
          buffer = buffer.slice(offset);
        }
      }
    } catch {
      // Stream error
    }
  }

  close(): void {
    this.subscriptions.clear();
    this.running = false;
  }
}
