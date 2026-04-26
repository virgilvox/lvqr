// Typed CustomEvent dispatch for the LVQR DVR player.
//
// All events bubble + are composed:false; detail payloads use the
// shapes documented on the LvqrDvrPlayerEvents map.

export interface LvqrDvrSeekDetail {
  fromTime: number;
  toTime: number;
  isLiveEdge: boolean;
  source: 'user' | 'programmatic';
}

export interface LvqrDvrLiveEdgeChangedDetail {
  isAtLiveEdge: boolean;
  deltaSecs: number;
  thresholdSecs: number;
}

export interface LvqrDvrErrorDetail {
  code: string;
  message: string;
  fatal: boolean;
  source: 'hls.js' | 'component';
}

export interface LvqrDvrPlayerEvents {
  'lvqr-dvr-seek': LvqrDvrSeekDetail;
  'lvqr-dvr-live-edge-changed': LvqrDvrLiveEdgeChangedDetail;
  'lvqr-dvr-error': LvqrDvrErrorDetail;
}

export function dispatchTyped<K extends keyof LvqrDvrPlayerEvents>(
  el: HTMLElement,
  name: K,
  detail: LvqrDvrPlayerEvents[K],
): void {
  el.dispatchEvent(new CustomEvent(name, { detail, bubbles: true, composed: false }));
}
