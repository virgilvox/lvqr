// Unit tests for @lvqr/dvr-player dispatch helper.
//
// Verifies that `dispatchTyped` produces a CustomEvent with the
// expected name, detail payload, bubbling, and composed flags.
// No DOM beyond a trivial EventTarget that the helper dispatches
// against; we use Node's built-in `EventTarget` (Node 16+).

import { describe, expect, it } from 'vitest';
import { dispatchTyped } from '../../packages/dvr-player/src/internals/dispatch';

class ElementShim extends EventTarget {}

describe('dispatchTyped', () => {
  it('dispatches lvqr-dvr-seek with the expected detail shape', () => {
    const el = new ElementShim() as unknown as HTMLElement;
    let captured: CustomEvent | null = null;
    el.addEventListener('lvqr-dvr-seek', (e) => {
      captured = e as CustomEvent;
    });
    dispatchTyped(el, 'lvqr-dvr-seek', {
      fromTime: 0,
      toTime: 30,
      isLiveEdge: false,
      source: 'user',
    });
    expect(captured).not.toBeNull();
    const event = captured as CustomEvent | null;
    expect(event?.type).toBe('lvqr-dvr-seek');
    expect(event?.bubbles).toBe(true);
    expect(event?.composed).toBe(false);
    expect(event?.detail).toEqual({
      fromTime: 0,
      toTime: 30,
      isLiveEdge: false,
      source: 'user',
    });
  });

  it('dispatches lvqr-dvr-live-edge-changed with the expected detail shape', () => {
    const el = new ElementShim() as unknown as HTMLElement;
    let captured: CustomEvent | null = null;
    el.addEventListener('lvqr-dvr-live-edge-changed', (e) => {
      captured = e as CustomEvent;
    });
    dispatchTyped(el, 'lvqr-dvr-live-edge-changed', {
      isAtLiveEdge: true,
      deltaSecs: 1.5,
      thresholdSecs: 6,
    });
    const event = captured as CustomEvent | null;
    expect(event?.detail).toEqual({
      isAtLiveEdge: true,
      deltaSecs: 1.5,
      thresholdSecs: 6,
    });
  });

  it('dispatches lvqr-dvr-error with the expected detail shape', () => {
    const el = new ElementShim() as unknown as HTMLElement;
    let captured: CustomEvent | null = null;
    el.addEventListener('lvqr-dvr-error', (e) => {
      captured = e as CustomEvent;
    });
    dispatchTyped(el, 'lvqr-dvr-error', {
      code: 'manifestLoadError',
      message: '404',
      fatal: true,
      source: 'hls.js',
    });
    const event = captured as CustomEvent | null;
    expect(event?.detail).toEqual({
      code: 'manifestLoadError',
      message: '404',
      fatal: true,
      source: 'hls.js',
    });
  });

  it('events bubble through the parent target chain', () => {
    // A CustomEvent with bubbles:true propagates through DOM
    // ancestors. Since EventTarget on its own has no parent chain,
    // we verify the event flag instead of the actual propagation
    // (which is exercised in the Playwright e2e against a real DOM).
    const el = new ElementShim() as unknown as HTMLElement;
    let bubblesFlag: boolean | null = null;
    el.addEventListener('lvqr-dvr-seek', (e) => {
      bubblesFlag = e.bubbles;
    });
    dispatchTyped(el, 'lvqr-dvr-seek', {
      fromTime: 0,
      toTime: 1,
      isLiveEdge: true,
      source: 'programmatic',
    });
    expect(bubblesFlag).toBe(true);
  });
});
