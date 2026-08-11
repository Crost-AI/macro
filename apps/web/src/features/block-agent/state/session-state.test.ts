import { describe, expect, it } from 'vitest';
import {
  initialSessionState,
  type QueuedPrompt,
  type SessionState,
  transition,
} from './session-state';

const prompt = (id: string): QueuedPrompt => ({ id, markdown: `p-${id}` });

const idle = initialSessionState;
const sending: SessionState = { phase: { type: 'sending' }, queue: [] };
const working: SessionState = { phase: { type: 'working' }, queue: [] };
const workingQueued: SessionState = {
  phase: { type: 'working' },
  queue: [prompt('q1'), prompt('q2')],
};

describe('transition: sending from idle', () => {
  it('posts the prompt and enters sending', () => {
    const result = transition(idle, {
      type: 'send_requested',
      prompt: prompt('a'),
    });
    expect(result.state.phase).toEqual({ type: 'sending' });
    expect(result.effects).toEqual([
      { type: 'post_prompts', prompts: [prompt('a')] },
    ]);
  });

  it('stays sending on post_succeeded — working is the fold’s call', () => {
    const result = transition(sending, { type: 'post_succeeded' });
    expect(result.state.phase).toEqual({ type: 'sending' });
    expect(result.effects).toEqual([]);
  });

  it('returns failed prompts to the queue and toasts on post_failed', () => {
    const result = transition(
      { phase: { type: 'sending' }, queue: [prompt('later')] },
      { type: 'post_failed', prompts: [prompt('a')] }
    );
    expect(result.state.phase).toEqual({ type: 'idle' });
    expect(result.state.queue).toEqual([prompt('a'), prompt('later')]);
    expect(result.effects[0]).toMatchObject({ type: 'toast' });
  });
});

describe('transition: queueing while a turn runs', () => {
  it('a send while working queues instead of posting', () => {
    const result = transition(working, {
      type: 'send_requested',
      prompt: prompt('q1'),
    });
    expect(result.state.phase).toEqual({ type: 'working' });
    expect(result.state.queue).toEqual([prompt('q1')]);
    expect(result.effects).toEqual([]);
  });

  it('a send while sending also queues', () => {
    const result = transition(sending, {
      type: 'send_requested',
      prompt: prompt('q1'),
    });
    expect(result.state.phase).toEqual({ type: 'sending' });
    expect(result.state.queue).toEqual([prompt('q1')]);
    expect(result.effects).toEqual([]);
  });

  it('queues preserve order across multiple sends', () => {
    const first = transition(working, {
      type: 'send_requested',
      prompt: prompt('q1'),
    });
    const second = transition(first.state, {
      type: 'send_requested',
      prompt: prompt('q2'),
    });
    expect(second.state.queue.map((q) => q.id)).toEqual(['q1', 'q2']);
  });
});

describe('transition: turn settling', () => {
  it('settling with an empty queue returns to idle', () => {
    const result = transition(working, { type: 'turn_settled' });
    expect(result.state).toEqual(idle);
    expect(result.effects).toEqual([]);
  });

  it('settling with a queue flushes the whole batch and enters sending', () => {
    const result = transition(workingQueued, { type: 'turn_settled' });
    expect(result.state.phase).toEqual({ type: 'sending' });
    expect(result.state.queue).toEqual([]);
    expect(result.effects).toEqual([
      { type: 'post_prompts', prompts: [prompt('q1'), prompt('q2')] },
    ]);
  });
});

describe('transition: collaborative turns', () => {
  it('a turn starting from idle (another collaborator) enters working', () => {
    const result = transition(idle, { type: 'turn_started' });
    expect(result.state.phase).toEqual({ type: 'working' });
  });

  it('a queued prompt survives a collaborator’s turn and flushes after it', () => {
    const afterStart = transition(
      { phase: { type: 'sending' }, queue: [prompt('mine')] },
      { type: 'turn_started' }
    );
    expect(afterStart.state.phase).toEqual({ type: 'working' });
    const afterSettle = transition(afterStart.state, { type: 'turn_settled' });
    expect(afterSettle.effects).toEqual([
      { type: 'post_prompts', prompts: [prompt('mine')] },
    ]);
  });
});

describe('transition: stop', () => {
  it('stop while working posts the interrupt but stays working', () => {
    const result = transition(working, { type: 'stop_requested' });
    expect(result.state.phase).toEqual({ type: 'working' });
    expect(result.effects).toEqual([{ type: 'post_stop' }]);
  });

  it('stop from idle is rejected without state change', () => {
    const result = transition(idle, { type: 'stop_requested' });
    expect(result.state).toEqual(idle);
    expect(result.effects).toEqual([]);
  });
});

describe('transition: mid-flush arrivals', () => {
  it('a post failure landing while working re-queues without leaving the turn', () => {
    const result = transition(
      { phase: { type: 'working' }, queue: [prompt('later')] },
      { type: 'post_failed', prompts: [prompt('a')] }
    );
    expect(result.state.phase).toEqual({ type: 'working' });
    expect(result.state.queue).toEqual([prompt('a'), prompt('later')]);
    expect(result.effects[0]).toMatchObject({ type: 'toast' });
  });

  it('a post success landing while working is a no-op', () => {
    const result = transition(working, { type: 'post_succeeded' });
    expect(result.state).toEqual(working);
    expect(result.effects).toEqual([]);
  });

  it('stop while sending posts the interrupt and stays sending', () => {
    const result = transition(sending, { type: 'stop_requested' });
    expect(result.state.phase).toEqual({ type: 'sending' });
    expect(result.effects).toEqual([{ type: 'post_stop' }]);
  });
});

describe('transition: fold races', () => {
  it('duplicate turn_started while working is a no-op', () => {
    const result = transition(workingQueued, { type: 'turn_started' });
    expect(result.state).toEqual(workingQueued);
  });

  it('turn_settled while idle is a no-op', () => {
    const result = transition(idle, { type: 'turn_settled' });
    expect(result.state).toEqual(idle);
  });
});
