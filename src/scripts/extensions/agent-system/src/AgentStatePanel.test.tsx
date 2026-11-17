import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, test } from '@rstest/core';

import { AgentStatePanel } from './AgentStatePanel';

const tr = (key: string) => key;
afterEach(cleanup);

function document(): TauriTavernAgentStateSelection {
    return {
        target: { chatRef: { kind: 'character', characterId: 'Alice', fileName: 'story' }, stableChatId: 'stable', messageIndex: 2, swipeId: 0 },
        document: { stateId: 'state-1', sha256: 'file-sha', chatSha256: 'chat-sha', readOnly: false, text: '{"player":{"money":123456789012345678901,"name":"Alice"}}', fields: [
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
    await user.click(await screen.findByRole('button', { name: /statePlayer/ }));
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

test('objects and arrays expand one level at a time and collapsed edits retain exact pointers and numbers', async () => {
    const loaded = document();
    loaded.document.text = '{"characters":{"0":{"name":"甲","resources":{"energy":123456789012345678901}},"a/b~c":{"name":"乙","state":"无伤"}},"inventory":[{"name":"钥匙","quantity":2}]}';
    loaded.document.fields = [
        { pointer: '/characters/0/name', kind: 'string', valueJson: '"甲"' },
        { pointer: '/characters/0/resources/energy', kind: 'number', valueJson: '123456789012345678901' },
        { pointer: '/characters/a~1b~0c/name', kind: 'string', valueJson: '"乙"' },
        { pointer: '/characters/a~1b~0c/state', kind: 'string', valueJson: '"无伤"' },
        { pointer: '/inventory/0/name', kind: 'string', valueJson: '"钥匙"' },
        { pointer: '/inventory/0/quantity', kind: 'number', valueJson: '2' },
    ];
    const saved: TauriTavernAgentStateSave[] = [];
    const api: TauriTavernAgentStateApi = { ...idle, readCurrent: () => Promise.resolve(loaded), save: input => {
        saved.push(input);
        return Promise.reject(new Error('keep draft'));
    } };
    render(<AgentStatePanel api={api} tr={tr} subscribeContext={subscribeContext} />);
    const user = userEvent.setup();
    await user.click(screen.getByRole('button', { name: 'stateTitle' }));
    const characters = await screen.findByRole('button', { name: /stateCharacters/ });
    expect(characters.getAttribute('aria-expanded')).toBe('false');
    expect(screen.queryAllByRole('textbox', { hidden: true })).toHaveLength(0);
    await user.click(characters);
    const first = screen.getByRole('button', { name: /甲/ });
    expect(first.textContent).not.toContain('1.');
    expect(screen.queryAllByRole('textbox', { hidden: true })).toHaveLength(0);
    await user.click(first);
    expect(screen.queryByRole('textbox', { name: 'characters / 0 / resources / energy', hidden: true })).toBeNull();
    await user.click(screen.getByRole('button', { name: /stateResources/ }));
    const energy = screen.getByRole('textbox', { name: 'characters / 0 / resources / energy' });
    expect((energy as HTMLTextAreaElement).value).toBe('123456789012345678901');
    await user.clear(energy);
    await user.type(energy, '123456789012345678902');
    await user.click(characters);
    expect(screen.queryByRole('textbox', { name: 'characters / 0 / resources / energy', hidden: true })).toBeNull();
    expect(characters.textContent).toContain('stateModified');
    await user.click(characters);
    expect(screen.getByRole<HTMLTextAreaElement>('textbox', { name: 'characters / 0 / resources / energy' }).value).toBe('123456789012345678902');
    await user.click(screen.getByRole('button', { name: /乙/ }));
    await user.clear(screen.getByRole('textbox', { name: 'characters / a/b~c / state' }));
    await user.type(screen.getByRole('textbox', { name: 'characters / a/b~c / state' }), '已休整');
    await user.click(screen.getByRole('button', { name: /stateInventory/ }));
    await user.click(screen.getByRole('button', { name: /1\. 钥匙/ }));
    expect(screen.getByRole<HTMLTextAreaElement>('textbox', { name: 'inventory / 0 / quantity' }).value).toBe('2');
    await user.click(screen.getByRole('button', { name: 'stateCollapseAll' }));
    expect(screen.queryAllByRole('textbox', { hidden: true })).toHaveLength(0);
    await user.click(screen.getByRole('button', { name: 'stateReview' }));
    await user.click(screen.getByRole('button', { name: 'stateSaveVersion' }));
    await screen.findByRole('alert');
    expect(saved[0]?.edits).toEqual([
        { pointer: '/characters/0/resources/energy', kind: 'number', valueJson: '123456789012345678902' },
        { pointer: '/characters/a~1b~0c/state', kind: 'string', valueJson: '"已休整"' },
    ]);
});

test('the state toggle stays where it was dragged across remounts and a drag does not open the panel', () => {
    localStorage.clear();
    const api: TauriTavernAgentStateApi = { ...idle, readCurrent: () => Promise.resolve(document()), save: () => Promise.reject(new Error('must not save')) };
    const first = render(<AgentStatePanel api={api} tr={tr} subscribeContext={subscribeContext} />);
    const toggle = screen.getByRole('button', { name: 'stateTitle' });
    fireEvent.pointerDown(toggle, { pointerId: 1, isPrimary: true, button: 0, clientX: 500, clientY: 400 });
    fireEvent.pointerMove(toggle, { pointerId: 1, isPrimary: true, clientX: 300, clientY: 250 });
    fireEvent.pointerUp(toggle, { pointerId: 1, isPrimary: true, clientX: 300, clientY: 250 });
    fireEvent.click(toggle, { detail: 1 });
    expect(toggle.getAttribute('aria-expanded')).toBe('false');
    const { left, top } = toggle.style;
    expect(left).not.toBe('');
    first.unmount();
    render(<AgentStatePanel api={api} tr={tr} subscribeContext={subscribeContext} />);
    const restored = screen.getByRole('button', { name: 'stateTitle' });
    expect([restored.style.left, restored.style.top]).toEqual([left, top]);
    localStorage.clear();
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
    await user.click(await screen.findByRole('button', { name: /statePlayer/ }));
    await waitFor(() => expect(screen.getByRole('textbox', { name: 'player / money' }).hasAttribute('disabled')).toBe(true));
    expect(screen.getByRole('button', { name: 'stateReview' }).hasAttribute('disabled')).toBe(true);
});
