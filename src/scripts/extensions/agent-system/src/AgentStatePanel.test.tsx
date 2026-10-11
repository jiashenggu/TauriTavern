import { act, cleanup, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, test } from '@rstest/core';

import { AgentStatePanel } from './AgentStatePanel';

const tr = (key: string) => key;
afterEach(cleanup);

function document(): TauriTavernAgentStateSelection {
    return {
        target: { chatRef: { kind: 'character', characterId: 'Alice', fileName: 'story' }, stableChatId: 'stable', messageIndex: 2, swipeId: 0 },
        document: { stateId: 'state-1', sha256: 'file-sha', chatSha256: 'chat-sha', readOnly: false, text: '{"player":{"money":123456789012345678901}}', fields: [
            { pointer: '/player/money', kind: 'number', valueJson: '123456789012345678901' },
            { pointer: '/player/name', kind: 'string', valueJson: '"Alice"' },
        ] },
    };
}

const idle = { isSaving: () => false, needsReload: () => false };
const subscribeContext = () => () => {};

test('state editor requires confirmation, preserves numeric text and retains drafts on failure', async () => {
    const saved: TauriTavernAgentStateSave[] = [];
    const api: TauriTavernAgentStateApi = { ...idle, readCurrent: () => Promise.resolve(document()), save: input => {
        saved.push(input);
        return Promise.reject(new Error('state conflict'));
    } };
    render(<AgentStatePanel api={api} tr={tr} subscribeContext={subscribeContext} />);
    const user = userEvent.setup();
    await user.click(screen.getByRole('button', { name: 'stateTitle' }));
    const money = await screen.findByRole('textbox', { name: 'player / money' });
    expect((money as HTMLTextAreaElement).value).toBe('123456789012345678901');
    await user.clear(money);
    await user.type(money, '123456789012345678902');
    await user.click(screen.getByRole('button', { name: 'stateReview' }));
    expect(saved).toHaveLength(0);
    await user.click(screen.getByRole('button', { name: 'stateSaveVersion' }));
    await screen.findByRole('alert');
    expect(saved[0]?.edits).toEqual([{ pointer: '/player/money', kind: 'number', valueJson: '123456789012345678902' }]);
    expect(saved[0]?.expectedStateId).toBe('state-1');
    expect(saved[0]?.expectedChatSha256).toBe('chat-sha');
    expect((money as HTMLTextAreaElement).value).toBe('123456789012345678902');
    await user.click(screen.getByRole('button', { name: 'cancel' }));
    expect((money as HTMLTextAreaElement).value).toBe('123456789012345678901');
});

test('a late state read cannot populate a different chat and running states are read-only', async () => {
    let changed: (() => void) | undefined;
    let finish: ((value: TauriTavernAgentStateSelection) => void) | undefined;
    let pending = true;
    const api: TauriTavernAgentStateApi = { ...idle,
        readCurrent: () => pending ? new Promise(resolve => { finish = resolve; }) : Promise.resolve({ ...document(), document: { ...document().document, readOnly: true } }),
        save: () => Promise.reject(new Error('must not save')),
    };
    render(<AgentStatePanel api={api} tr={tr} subscribeContext={listener => { changed = listener; return () => {}; }} />);
    const user = userEvent.setup();
    await user.click(screen.getByRole('button', { name: 'stateTitle' }));
    act(() => changed?.());
    await act(() => { finish?.(document()); return Promise.resolve(); });
    expect(screen.queryByRole('textbox', { name: 'player / money' })).toBeNull();
    expect(screen.getByRole('alert').textContent).toBe('stateContextChanged');
    pending = false;
    await user.click(screen.getByRole('button', { name: 'refresh' }));
    await waitFor(() => expect(screen.getByRole('textbox', { name: 'player / money' }).hasAttribute('disabled')).toBe(true));
    expect(screen.getByRole('button', { name: 'stateReview' }).hasAttribute('disabled')).toBe(true);
});
