import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';
import ts from 'typescript';

const streamSource = readFileSync(new URL('../src/scripts/util/stream-fadein.js', import.meta.url), 'utf8');
const scriptSource = readFileSync(new URL('../src/script.js', import.meta.url), 'utf8');
const ast = ts.createSourceFile('script.js', scriptSource, ts.ScriptTarget.Latest, true, ts.ScriptKind.JS);
const processorCode = ast.statements.find(node => ts.isClassDeclaration(node) && node.name.text === 'StreamingProcessor').getText(ast);
const streamCode = streamSource.replace(/^import .*;\r?\n/, '').replace(/^export /gm, '');

function fixture(type = 'normal', rendered = true) {
    const drafts = [
        { dataset: {}, open: true },
        { dataset: { streamingUserOpen: 'true' }, open: true },
        { dataset: { streamingUserOpen: 'false' }, open: false },
    ];
    const events = [];
    class Element {}
    const host = Object.assign(new Element(), {
        querySelectorAll(selector) {
            assert.equal(selector, 'details[data-streaming-open], details.draft-notes, details.custom-draft-notes');
            return drafts;
        },
    });
    Object.defineProperty(host, 'innerHTML', {
        get: () => 'Existing partial story',
        set() { assert.fail('Error cleanup must not rewrite the message'); },
    });
    let unlocked = false;
    const flushed = [];
    const context = vm.createContext({
        AbortController, HTMLElement: Element,
        document: { querySelector: () => null },
        ReasoningHandler: class {
            flushPendingRender(messageId) { flushed.push(messageId); }
            finish() { assert.fail('An error must not finalize reasoning'); }
        },
        Stopwatch: class {}, power_user: { streaming_fps: 30 },
        main_api: 'openai', getStoppingStrings: () => [],
        console: { error() {} },
        unblockGeneration() { unlocked = true; },
        event_types: { MESSAGE_RECEIVED: 'received', CHARACTER_MESSAGE_RENDERED: 'rendered' },
        eventSource: { emit(event) { events.push({ event, open: drafts[0].open }); } },
    });
    vm.runInContext(streamCode + '\n' + processorCode + '\nglobalThis.Processor = StreamingProcessor;', context);
    const processor = new context.Processor(type, false, new Date(), '', {});
    processor.messageId = 0;
    processor.messageTextDom = rendered ? host : null;
    processor.result = 'Existing partial story';
    processor.generator = async function* () { throw new Error('Synthetic stream disconnect'); };
    processor.onFinishStreaming = () => assert.fail('An error must not use successful completion');
    return { processor, drafts, host, events, flushed, unlocked: () => unlocked };
}

for (const type of ['normal', 'swipe', 'continue']) {
    test(`${type}: stream failure collapses only untouched drafts before emitting terminal events`, async () => {
        const f = fixture(type);
        assert.equal(await f.processor.generate(), 'Existing partial story');
        assert.equal(f.processor.isStopped, true);
        assert.equal(f.processor.isFinished, false);
        assert.equal(f.processor.abortController.signal.aborted, true);
        assert.equal(f.unlocked(), true);
        assert.deepEqual(f.flushed, [0]);
        assert.deepEqual(f.drafts.map(detail => detail.open), [false, true, false]);
        assert.deepEqual(f.drafts.map(detail => detail.dataset.streamingUserOpen), [undefined, 'true', 'false']);
        assert.equal(f.host.innerHTML, 'Existing partial story');
        assert.deepEqual(f.events, type === 'normal' ? [{ event: 'received', open: false }, { event: 'rendered', open: false }] : []);
        f.processor.onErrorStreaming();
        assert.deepEqual(f.drafts.map(detail => detail.open), [false, true, false]);
    });
}

test('a failed reasoning render does not prevent stream error cleanup', async () => {
    const f = fixture();
    f.processor.reasoningHandler.flushPendingRender = () => { throw new Error('Synthetic formatter failure'); };
    assert.equal(await f.processor.generate(), 'Existing partial story');
    assert.equal(f.unlocked(), true);
    assert.equal(f.processor.isStopped, true);
    assert.equal(f.processor.abortController.signal.aborted, true);
    assert.deepEqual(f.drafts.map(detail => detail.open), [false, true, false]);
    assert.deepEqual(f.events, [{ event: 'received', open: false }, { event: 'rendered', open: false }]);
});

for (const type of ['normal', 'impersonate']) {
    test(`${type}: stream failure before message rendering still unlocks generation`, async () => {
        const f = fixture(type, false);
        await f.processor.generate();
        assert.equal(f.processor.isStopped, true);
        assert.equal(f.unlocked(), true);
        assert.deepEqual(f.flushed, [0]);
        assert.deepEqual(f.drafts.map(detail => detail.open), [true, true, false]);
    });
}
