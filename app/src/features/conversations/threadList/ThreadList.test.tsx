/**
 * Accessible-name tests for the chat sidebar's thread rows.
 *
 * Every row carries the same pair of icon-only buttons (rename, delete), so a
 * label that names only the verb leaves a screen reader with N indistinguishable
 * controls. The rename button used to be `aria-label="Edit thread title"` on
 * every row and the delete button carried no `aria-label` at all.
 */
import { render, screen } from '@testing-library/react';
import { createRef } from 'react';
import { describe, expect, it, vi } from 'vitest';

import type { Thread } from '../../../types/thread';
import { ThreadList } from './ThreadList';

// `ThreadList` imports one helper from the Conversations page module; pulling
// the whole page in would make this a page test rather than a component one.
vi.mock('../Conversations', () => ({ isImeCompositionKeyEvent: () => false }));

function makeThread(overrides: Partial<Thread> = {}): Thread {
  return {
    id: 't-1',
    title: 'Quarterly report',
    chatId: null,
    isActive: false,
    messageCount: 0,
    lastMessageAt: '2026-01-01T00:00:00.000Z',
    createdAt: '2026-01-01T00:00:00.000Z',
    labels: [],
    ...overrides,
  };
}

function renderList(threads: Thread[]) {
  const props = {
    threads,
    selectedThreadId: null,
    onCreateThread: vi.fn(),
    onSelectThread: vi.fn(),
    resolveTitle: (id: string) => threads.find(t => t.id === id)?.title ?? id,
    onRequestDelete: vi.fn(),
    editingThreadId: null,
    editTitleValue: '',
    editTitleInputRef: createRef<HTMLInputElement>(),
    onEditTitleValueChange: vi.fn(),
    onStartEditTitle: vi.fn(),
    onCommitTitle: vi.fn(),
    onCancelEditTitle: vi.fn(),
    onBlurTitle: vi.fn(),
  };
  return { props, ...render(<ThreadList {...props} />) };
}

describe('ThreadList row actions', () => {
  it('names the rename and delete buttons after the thread they act on', () => {
    renderList([makeThread()]);

    expect(
      screen.getByRole('button', { name: 'Rename conversation Quarterly report' })
    ).toBeInTheDocument();
    expect(
      screen.getByRole('button', { name: 'Delete conversation Quarterly report' })
    ).toBeInTheDocument();
  });

  it('gives every row a distinct accessible name for each action', () => {
    renderList([
      makeThread({ id: 't-1', title: 'Quarterly report' }),
      makeThread({ id: 't-2', title: 'Trip planning' }),
    ]);

    // If the labels were shared, `getByRole` would throw on multiple matches.
    const names = screen
      .getAllByRole('button')
      .map(b => b.getAttribute('aria-label'))
      .filter((n): n is string => n != null);

    expect(names).toEqual(
      expect.arrayContaining([
        'Rename conversation Quarterly report',
        'Delete conversation Quarterly report',
        'Rename conversation Trip planning',
        'Delete conversation Trip planning',
      ])
    );
    expect(new Set(names).size).toBe(names.length);
  });

  it('drives the row callbacks from those buttons', async () => {
    const { props } = renderList([makeThread()]);

    screen.getByRole('button', { name: 'Rename conversation Quarterly report' }).click();
    expect(props.onStartEditTitle).toHaveBeenCalledWith('t-1');

    screen.getByRole('button', { name: 'Delete conversation Quarterly report' }).click();
    expect(props.onRequestDelete).toHaveBeenCalledWith(expect.objectContaining({ id: 't-1' }));
  });
});
