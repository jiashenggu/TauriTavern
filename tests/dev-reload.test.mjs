import assert from 'node:assert/strict';
import test from 'node:test';
import { installDevReload } from '../src/dev-reload.js';

const flush = () => new Promise(resolve => setImmediate(resolve));
function fixture() {
    let reloads = 0;
    let generation = false;
    let saving = false;
    let activeRunIds = [];
    let query = async () => ({ activeRunIds });
    let flushSave = () => null;
    let source;
    const timers = new Map();
    let timerId = 0;
    const warnings = [];
    const script = {
        isGenerating: () => generation,
        get isChatSaving() { return saving; },
        flushDebouncedChatSave: () => flushSave(),
    };
    const scope = Object.assign(new EventTarget(), {
        document: { body: { dataset: {} } },
        location: { reload: () => { reloads++; } },
        console: { info() {}, warn: (...args) => warnings.push(args) },
        setTimeout: callback => { timers.set(++timerId, callback); return timerId; },
        clearTimeout: id => timers.delete(id),
        EventSource: class extends EventTarget { constructor() { super(); source = this; } close() {} },
        __TAURI_INTERNALS__: {},
        __TAURITAVERN__: { api: { agent: { readActivity: () => query(), state: { isSaving: () => false, needsReload: () => false } } } },
    });
    const stop = installDevReload('http://localhost:1430/__tauritavern_dev_reload', { scope, loadScript: async () => script });
    return { scope, stop, warnings, get reloads() { return reloads; },
        get timerCount() { return timers.size; },
        generation: value => { generation = value; }, saving: value => { saving = value; },
        pendingRuns: value => { activeRunIds = value; }, query: value => { query = value; },
        flushSave: value => { flushSave = value; },
        async reload() { source.dispatchEvent(new Event('reload')); await flush(); },
        async runState(detail) { scope.dispatchEvent(new CustomEvent('tauritavern-agent-run-state-changed', { detail })); await flush(); },
        async tick() { const next = timers.entries().next().value; assert.ok(next, 'deferred check must be scheduled'); timers.delete(next[0]); await next[1](); await flush(); },
    };
}

test('reload drains debounced saves and rechecks activity and new edits after each save', async t => {
    const f = fixture();
    t.after(f.stop);
    let finishFirst;
    let finishSecond;
    const first = new Promise(resolve => { finishFirst = resolve; });
    const second = new Promise(resolve => { finishSecond = resolve; });
    const queued = [first];
    let saves = 0;
    f.flushSave(() => {
        const save = queued.shift() ?? null;
        if (save) saves++;
        return save;
    });
    // The debounce timer has not fired, so isChatSaving is still false.
    await f.reload();
    await f.reload();
    assert.equal(saves, 1);
    assert.equal(f.reloads, 0);

    // Another edit and a backend run arrive while the first save is pending.
    queued.push(second);
    f.pendingRuns(['new-run']);
    finishFirst();
    await flush();
    assert.equal(saves, 1);
    assert.equal(f.reloads, 0);
    f.pendingRuns([]);
    const retry = f.tick();
    await flush();
    assert.equal(saves, 2);
    assert.equal(f.reloads, 0);

    f.generation(true);
    finishSecond();
    await retry;
    assert.equal(f.reloads, 0);
    f.generation(false);
    await f.tick();
    assert.equal(f.reloads, 1);
    assert.equal(queued.length, 0);
});

test('a failed flush blocks subsequent automatic reloads even after its pending task is consumed', async t => {
    for (const synchronous of [false, true]) {
        await t.test(synchronous ? 'synchronous failure' : 'rejected save', async t => {
            const f = fixture();
            t.after(f.stop);
            const failure = new Error('chat save failed');
            f.flushSave(() => {
                f.flushSave(() => null);
                if (synchronous) throw failure;
                return Promise.reject(failure);
            });
            await f.reload();
            assert.equal(f.reloads, 0);
            assert.equal(f.warnings.length, 1);
            assert.equal(f.warnings[0][1], failure);
            assert.match(f.warnings[0][0], /reload manually/);
            assert.equal(f.timerCount, 0);
            await f.reload();
            await f.runState({ activeRun: null });
            assert.equal(f.reloads, 0);
            assert.equal(f.timerCount, 0);
        });
    }
});

test('disposal while flushing a debounced save prevents navigation', async () => {
    const f = fixture();
    let finish;
    f.flushSave(() => {
        f.flushSave(() => null);
        return new Promise(resolve => { finish = resolve; });
    });
    await f.reload();
    assert.equal(typeof finish, 'function');
    f.stop();
    finish();
    await flush();
    assert.equal(f.reloads, 0);
    assert.equal(f.timerCount, 0);
});

test('hot reload waits through generation, host commit and presentation settlement', async () => {
    const f = fixture();
    await f.runState({ activeRun: { runId: 'run' } });
    await f.reload();
    await f.reload();
    assert.equal(f.reloads, 0);
    f.pendingRuns(['awaiting-host-commit']);
    await f.runState({ activeRun: null });
    assert.equal(f.reloads, 0);
    f.pendingRuns([]);
    await f.runState({ activeRun: null, presentationError: 'acknowledgement lost' });
    await f.tick();
    assert.equal(f.reloads, 0);
    f.saving(true);
    await f.runState({ activeRun: null, presentationError: '' });
    assert.equal(f.reloads, 0);
    f.saving(false);
    await f.tick();
    assert.equal(f.reloads, 1);
    f.stop();
});

test('reload rechecks generation after an asynchronous backend probe and refuses unknown outcomes', async () => {
    const f = fixture();
    let finish;
    f.query(() => new Promise(resolve => { finish = resolve; }));
    await f.reload();
    f.generation(true);
    finish({ activeRunIds: [] });
    await flush();
    assert.equal(f.reloads, 0);
    f.generation(false);
    f.query(async () => { throw new Error('backend unavailable'); });
    await f.tick();
    assert.equal(f.reloads, 0);
    assert.equal(f.warnings.length, 1);
    for (const result of [{}, { activeRunIds: null }, { activeRunIds: [null] }]) {
        f.query(async () => result);
        await f.tick();
        assert.equal(f.reloads, 0);
    }
    f.query(async () => ({ activeRunIds: [] }));
    await f.tick();
    assert.equal(f.reloads, 1);
    f.stop();
});

test('a reloaded frontend still checks backend runs and disposal cancels an in-flight reload', async () => {
    const f = fixture();
    f.pendingRuns(['finishing']);
    await f.reload();
    assert.equal(f.reloads, 0);
    let finish;
    f.query(() => new Promise(resolve => { finish = resolve; }));
    const pending = f.tick();
    await flush();
    f.stop();
    finish({ activeRunIds: [] });
    await pending;
    assert.equal(f.reloads, 0);
});

test('interrupted history does not block reload when the runtime is idle', async () => {
    const f = fixture();
    f.scope.__TAURITAVERN__.api.agent.listRuns = async () => ({
        runs: [{ runId: 'interrupted', status: 'calling_model' }],
    });
    await f.reload();
    assert.equal(f.reloads, 1);
    f.stop();
});
