// Synthetic RTMP push helper for live-stream-driven Playwright
// suites.
//
// Spawns ffmpeg as a child process to push a deterministic
// testsrc + sine RTMP feed at the relay. The helper returns a
// control handle whose `stop()` SIGTERMs ffmpeg cleanly so the
// test's afterEach does not leak a process. ffmpeg's own
// `-t <secs>` flag also caps the runtime so a forgotten stop()
// still terminates within the watch window.
//
// Closes session 153's deferred "live-stream-driven Playwright
// assertions" item: the session-153 spec used DOM-stub
// `Object.defineProperty(v, 'seekable', ...)` because no helper
// existed; with this helper, a Playwright spec can mount the
// dvr-player against a real publishing relay and assert against
// genuine `seekable.end - currentTime` deltas.
//
// CI parity: the mesh-e2e workflow at
// `.github/workflows/mesh-e2e.yml` does NOT install ffmpeg today;
// callers should `await rtmpPushAvailable()` and Playwright
// `test.skip()` when ffmpeg is missing so the helper is opt-in
// per spec.

import { spawn, type ChildProcess } from 'node:child_process';
import { existsSync } from 'node:fs';

export interface RtmpPushOptions {
  /** Full RTMP URL incl. broadcast key, e.g. rtmp://127.0.0.1:11936/live/dvr-test. */
  rtmpUrl: string;
  /** Total stream runtime in seconds (ffmpeg -t). */
  durationSecs: number;
  /** Video bitrate in kbit/s. Default 1500. */
  videoBitrateK?: number;
  /** Frame rate in Hz. Default 30. */
  frameRate?: number;
  /** Resolution as WxH. Default "320x180". */
  size?: string;
  /** Path to ffmpeg. Default "ffmpeg" (PATH lookup). */
  ffmpegPath?: string;
  /** stdout / stderr handlers for diagnostics. Default no-op. */
  onStdout?: (chunk: string) => void;
  onStderr?: (chunk: string) => void;
}

export interface RtmpPushHandle {
  /** Kill the ffmpeg child cleanly. Resolves on exit. */
  stop: () => Promise<void>;
  /** Underlying child process (for advanced control). */
  child: ChildProcess;
  /** Promise that resolves when ffmpeg exits (with the exit code). */
  exited: Promise<number>;
}

/**
 * Returns true when an `ffmpeg` binary is reachable on the
 * caller's machine. Cheap: a `which`-style PATH lookup. Use in
 * `test.beforeAll` to gate the spec on ffmpeg availability.
 */
export function rtmpPushAvailable(ffmpegPath: string = 'ffmpeg'): boolean {
  if (ffmpegPath.includes('/')) return existsSync(ffmpegPath);
  // PATH lookup via spawnSync.
  try {
    // Lazy require to avoid pulling node:child_process for
    // callers that just want the type. Node's spawn already
    // pulled it though, so this is free.
    const { spawnSync } = require('node:child_process') as typeof import('node:child_process');
    const r = spawnSync('sh', ['-c', `command -v ${ffmpegPath}`], { stdio: 'pipe' });
    return r.status === 0 && r.stdout.toString().trim().length > 0;
  } catch {
    return false;
  }
}

/**
 * Spawn ffmpeg in the background to push a synthetic RTMP feed
 * at the supplied URL. Resolves once the child process is
 * spawned (NOT once the relay confirms publish; callers should
 * poll the relay's playlist for readiness).
 *
 * The synthetic feed is a `testsrc` video (color bars + a
 * frame counter) plus a 440 Hz sine tone, encoded as H.264 +
 * AAC at the requested bitrate. Encoder options favour
 * deterministic, low-CPU output:
 * `-preset ultrafast -tune zerolatency -g <2*frameRate>`.
 */
export function rtmpPush(opts: RtmpPushOptions): RtmpPushHandle {
  const ffmpegPath = opts.ffmpegPath ?? 'ffmpeg';
  const fps = opts.frameRate ?? 30;
  const size = opts.size ?? '320x180';
  const vbk = opts.videoBitrateK ?? 1500;
  const args = [
    '-hide_banner',
    '-loglevel', 'info',
    '-re',
    '-f', 'lavfi',
    '-i', `testsrc=size=${size}:rate=${fps}`,
    '-f', 'lavfi',
    '-i', 'sine=frequency=440',
    '-c:v', 'libx264',
    '-preset', 'ultrafast',
    '-tune', 'zerolatency',
    '-g', String(Math.max(1, Math.floor(fps * 2))),
    '-b:v', `${vbk}k`,
    '-pix_fmt', 'yuv420p',
    '-c:a', 'aac',
    '-ar', '44100',
    '-b:a', '128k',
    '-shortest',
    '-t', String(opts.durationSecs),
    '-f', 'flv',
    opts.rtmpUrl,
  ];
  // stdio: stdout/stderr are inherited so OS pipe buffers
  // never fill (a backed-up pipe would block ffmpeg mid-encode
  // and manifest as "ffmpeg never connected to the relay").
  // Optional onStdout / onStderr handlers are wired by piping
  // selectively when the caller supplies them.
  const stdio: Array<'ignore' | 'pipe' | 'inherit'> = [
    'ignore',
    opts.onStdout ? 'pipe' : 'inherit',
    opts.onStderr ? 'pipe' : 'inherit',
  ];
  const child = spawn(ffmpegPath, args, { stdio });

  if (opts.onStdout) child.stdout?.on('data', (c: Buffer) => opts.onStdout?.(c.toString('utf8')));
  if (opts.onStderr) child.stderr?.on('data', (c: Buffer) => opts.onStderr?.(c.toString('utf8')));

  const exited: Promise<number> = new Promise((resolve) => {
    child.on('exit', (code) => resolve(code ?? -1));
  });

  const stop = async (): Promise<void> => {
    if (child.exitCode !== null) return;
    try {
      child.kill('SIGTERM');
    } catch {
      // already exited
    }
    // Hard-kill after a short grace window so a misbehaving
    // ffmpeg never blocks afterEach.
    const timer = setTimeout(() => {
      if (child.exitCode === null) {
        try {
          child.kill('SIGKILL');
        } catch {
          // ignore
        }
      }
    }, 2_000);
    await exited;
    clearTimeout(timer);
  };

  return { stop, child, exited };
}
