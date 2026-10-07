import assert from 'node:assert/strict';
import test from 'node:test';

import { createShujukuTaskScopes } from '../scripts/patch-shujuku-lifecycle.mjs';

function environment() {
    const first = { is_user: false, mes: 'first reply', swipe_id: 0 };
    const second = { is_user: false, mes: 'second reply', swipe_id: 0 };
    let context = { chatId: 'original', isolationKey: '', chat: [first, second] };
    const tasks = createShujukuTaskScopes(() => context);
    return { tasks, get context() { return context; }, set context(value) { context = value; } };
}

function deferred() {
    let resolve;
    const promise = new Promise(done => { resolve = done; });
    return { promise, resolve };
}

const isStale = error => error.name === 'AbortError' && error.code === 'table_task_stale';

test('chat switch cancels all in-flight groups and rejects a late result even if the provider ignores abort', async () => {
    const env = environment();
    const pending = deferred();
    const first = env.tasks.capture([0]);
    const second = env.tasks.capture([1]);
    const a = new AbortController();
    const b = new AbortController();
    first.track(a);
    second.track(b);
    let committed = false;
    const result = (async () => {
        await pending.promise;
        first.assertCurrent();
        committed = true;
    })();
    env.context = { chatId: 'branch', isolationKey: '', chat: [{ is_user: false, mes: 'different reply' }] };
    env.tasks.invalidate();
    assert.equal(a.signal.aborted, true);
    assert.equal(b.signal.aborted, true);
    pending.resolve();
    await assert.rejects(result, isStale);
    assert.equal(committed, false);
});

test('normal concurrent groups survive each other writing table metadata', () => {
    const env = environment();
    const first = env.tasks.capture([0]);
    const second = env.tasks.capture([1]);
    env.context.chat[0].TavernDB_ACU_IsolatedData = { checkpoint: {} };
    first.assertCurrent();
    second.assertCurrent();
    first.dispose();
    second.assertCurrent();
});

test('switching away and back cannot revive an old task even if the same chat array is reused', () => {
    const env = environment();
    const original = env.context;
    const scope = env.tasks.capture([0]);
    env.tasks.invalidate();
    env.context = { chatId: 'other', isolationKey: '', chat: [] };
    env.tasks.invalidate();
    env.context = original;
    assert.throws(() => scope.assertCurrent(), isStale);
});

test('target deletion or replacement at the same index cancels the task before writes', () => {
    for (const replace of [false, true]) {
        const env = environment();
        const scope = env.tasks.capture([1]);
        const controller = new AbortController();
        scope.track(controller);
        if (replace) env.context.chat[1] = { ...env.context.chat[1] };
        else env.context.chat.pop();
        assert.throws(() => scope.assertCurrent(), isStale);
        assert.equal(controller.signal.aborted, true);
    }
});

test('editing or swiping a bound reply invalidates its generated result', () => {
    for (const field of ['mes', 'swipe_id']) {
        const env = environment();
        const scope = env.tasks.capture([0]);
        env.context.chat[0][field] = field === 'mes' ? 'edited reply' : 1;
        assert.throws(() => scope.assertCurrent(), isStale);
    }
});

test('an isolation change is stale even without a chat changed event', () => {
    const env = environment();
    const scope = env.tasks.capture([0]);
    env.context.isolationKey = 'another-slot';
    assert.throws(() => scope.assertCurrent(), isStale);
});

test('a cancelled parent cannot launch another batch or capture fresh targets in the new chat', () => {
    const env = environment();
    const parent = env.tasks.capture([0, 1]);
    env.tasks.invalidate();
    assert.throws(() => env.tasks.capture([1], parent), isStale);
});

test('finished scopes release controllers and manual cancellation does not cancel unrelated groups', () => {
    const env = environment();
    const first = env.tasks.capture([0]);
    const second = env.tasks.capture([1]);
    const a = new AbortController();
    const b = new AbortController();
    first.track(a);
    second.track(b);
    a.abort();
    assert.throws(() => first.assertCurrent(), isStale);
    second.assertCurrent();
    assert.equal(b.signal.aborted, false);
    second.dispose();
    env.tasks.invalidate();
    assert.equal(b.signal.aborted, false);
});
