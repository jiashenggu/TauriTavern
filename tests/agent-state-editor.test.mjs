import assert from 'node:assert/strict';
import test from 'node:test';
import { createAgentStateApi } from '../src/tauri/main/api/agent-state.js';

function fixture(invoke) {
    const target = { chatRef: { kind: 'character', characterId: 'Alice', fileName: 'story.jsonl' }, stableChatId: 'stable', messageIndex: 0, swipeId: 0 };
    const extra = stateId => ({ custom: 'keep', tauritavern: { agent: { runId: 'run', persistStateStatus: 'committed', persistStateId: stateId } } });
    const message = { mes: 'Keep body', swipe_id: 0, extra: extra('original'), swipe_info: [{ extra: extra('original') }, { extra: extra('other') }] };
    let chatRef = target.chatRef;
    const script = { chat: [message], isGenerating: () => false, enqueueChatSave: task => task() };
    globalThis.window = Object.assign(new EventTarget(), { __TAURITAVERN__: { api: { chat: {
        current: { ref: () => chatRef }, open: () => ({ stableId: () => Promise.resolve('stable') }),
    } } } });
    const api = createAgentStateApi({ safeInvoke: invoke, loadScript: async () => script });
    return { api, script, message, target, switchChat: () => { chatRef = { ...chatRef, fileName: 'different.jsonl' }; } };
}

function saveInput(target) {
    return { target, expectedStateId: 'original', expectedSha256: 'file-sha', expectedChatSha256: 'chat-sha', edits: [{ pointer: '/balance', kind: 'number', valueJson: '123456789012345678902' }] };
}

test('state read is read-only and save only changes the selected Agent metadata', async () => {
    const calls = [];
    const f = fixture(async (command, args) => {
        calls.push({ command, args });
        if (command === 'read_agent_state_document') return { stateId: 'original', sha256: 'file-sha', chatSha256: 'chat-sha', text: '{"balance":123456789012345678901}', fields: [], readOnly: false };
        assert.equal(command, 'save_agent_state_document');
        assert.equal(args.dto.edits[0].valueJson, '123456789012345678902');
        return { stateId: 'edited', agentMetadata: { manualStateEdit: { editId: args.dto.editId } } };
    });
    const read = await f.api.readCurrent();
    assert.equal(read.document.text, '{"balance":123456789012345678901}');
    const otherSwipe = structuredClone(f.message.swipe_info[1]);
    const input = saveInput(f.target);
    input.target = { ...input.target, chatRef: { fileName: 'story.jsonl', characterId: 'Alice', kind: 'character' } };
    await f.api.save(input);
    assert.deepEqual(calls.map(call => call.command), ['read_agent_state_document', 'save_agent_state_document']);
    assert.equal(f.message.mes, 'Keep body');
    assert.equal(f.message.extra.custom, 'keep');
    assert.equal(f.message.extra.tauritavern.agent.persistStateId, 'edited');
    assert.deepEqual(f.message.swipe_info[0].extra, f.message.extra);
    assert.deepEqual(f.message.swipe_info[1], otherSwipe);
    assert.equal(f.api.isSaving(), false);
});

test('lost save acknowledgement is recovered without reapplying edits', async () => {
    let saves = 0;
    let editId;
    const f = fixture(async (command, args) => {
        if (command === 'save_agent_state_document') {
            saves++;
            editId = args.dto.editId;
            throw new Error('acknowledgement lost');
        }
        return { stateId: 'edited', manualEdit: { editId } };
    });
    const result = await f.api.save(saveInput(f.target));
    assert.equal(result.stateId, 'edited');
    assert.equal(saves, 1);
    assert.equal(f.message.extra.tauritavern.agent.persistStateId, 'edited');
    assert.equal(f.api.needsReload(), false);
});

test('known validation rejection releases the gate without requiring a recovery read', async () => {
    let calls = 0;
    const f = fixture(async () => { calls++; throw new Error('Bad request: agent.state_invalid: workspace has an active run'); });
    await assert.rejects(f.api.save(saveInput(f.target)), /active run/);
    assert.equal(calls, 1);
    assert.equal(f.api.isSaving(), false);
    assert.equal(f.api.needsReload(), false);
});

test('unknown save outcomes block further edits and generation until application reload', async () => {
    const f = fixture(async () => { throw new Error('transport unavailable'); });
    await assert.rejects(f.api.save(saveInput(f.target)), /state_save_unknown/);
    assert.equal(f.api.needsReload(), true);
    assert.equal(f.api.isSaving(), true);
    assert.throws(() => f.api.assertIdle(), /state_save_unknown/);
    await assert.rejects(f.api.readCurrent(), /state_save_unknown/);
    assert.equal(f.message.extra.tauritavern.agent.persistStateId, 'original');
});

test('queued saves reject a changed chat and refuse concurrent generation', async () => {
    let sent = 0;
    const f = fixture(async () => { sent++; });
    f.script.isGenerating = () => true;
    await assert.rejects(f.api.save(saveInput(f.target)), /state_busy/);
    f.script.isGenerating = () => false;
    f.script.enqueueChatSave = async task => { f.switchChat(); return task(); };
    await assert.rejects(f.api.save(saveInput(f.target)), /state_conflict/);
    assert.equal(sent, 0);
});

test('a swipe selected during saving does not receive another swipe state', async () => {
    const f = fixture(async () => {
        f.message.swipe_id = 1;
        f.message.extra = structuredClone(f.message.swipe_info[1].extra);
        return { stateId: 'edited', agentMetadata: { manualStateEdit: { editId: 'edit' } } };
    });
    await f.api.save(saveInput(f.target));
    assert.equal(f.message.extra.tauritavern.agent.persistStateId, 'other');
    assert.equal(f.message.swipe_info[0].extra.tauritavern.agent.persistStateId, 'edited');
});
