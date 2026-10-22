import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';
import ts from 'typescript';
import { assertGenerationOutput } from '../src/scripts/util/generation-lifecycle.js';
import { getStreamingRenderInterval, normalizeStreamingFps } from '../src/scripts/tauri/perf/streaming-render-policy.js';

const streamSource = readFileSync(new URL('../src/scripts/util/stream-fadein.js', import.meta.url), 'utf8');
const scriptSource = readFileSync(new URL('../src/script.js', import.meta.url), 'utf8');
const ast = ts.createSourceFile('script.js', scriptSource, ts.ScriptTarget.Latest, true, ts.ScriptKind.JS);
const processorCode = ast.statements.find(node => ts.isClassDeclaration(node) && node.name.text === 'StreamingProcessor').getText(ast);
const streamCode = streamSource.replace(/^import .*;\r?\n/, '').replace(/^export /gm, '');
const openaiAst = ts.createSourceFile('openai.js',
    readFileSync(new URL('../src/scripts/openai.js', import.meta.url), 'utf8'),
    ts.ScriptTarget.Latest, true, ts.ScriptKind.JS);
const replyCode = openaiAst.statements.find(node => ts.isFunctionDeclaration(node) && node.name.text === 'getStreamingReply')
    .getText(openaiAst).replace(/^export /, '');
const toolAst = ts.createSourceFile('tool-calling.js',
    readFileSync(new URL('../src/scripts/tool-calling.js', import.meta.url), 'utf8'),
    ts.ScriptTarget.Latest, true, ts.ScriptKind.JS);
const toolClass = toolAst.statements.find(node => ts.isClassDeclaration(node) && node.name.text === 'ToolManager');
const toolCode = 'class ToolManager {' + toolClass.members
    .filter(node => ['getToolCallsFromData', 'hasToolCalls'].includes(node.name?.getText(toolAst)))
    .map(node => node.getText(toolAst)).join('\n') + '}';

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
        AbortController, HTMLElement: Element, assertGenerationOutput,
        getStreamingRenderInterval, normalizeStreamingFps,
        document: { querySelector: () => null },
        ReasoningHandler: class {
            flushPendingRender(messageId) { flushed.push(messageId); }
            updateReasoning(_messageId, reasoning) { this.reasoning = reasoning; }
            finish() { assert.fail('An error must not finalize reasoning'); }
        },
        Stopwatch: class { async tick(callback) { await callback(); } }, power_user: { streaming_fps: 30 },
        main_api: 'openai', getStoppingStrings: () => [],
        getPromptCacheUsage: () => null,
        console: { error() {}, warn() {} },
        unblockGeneration() { unlocked = true; },
        async finalizeMessageContent(_messageId, eventType) {
            if (eventType) events.push({ event: eventType, open: drafts[0].open });
        },
        event_types: { MESSAGE_RECEIVED: 'received', CHARACTER_MESSAGE_RENDERED: 'rendered', STREAM_TOKEN_RECEIVED: 'token' },
        eventSource: { emit(event) { if (event !== 'token') events.push({ event, open: drafts[0].open }); } },
    });
    vm.runInContext(toolCode + '\n' + streamCode + '\n' + processorCode + '\nglobalThis.Processor = StreamingProcessor;', context);
    const processor = new context.Processor(type, false, new Date(), '', {});
    processor.messageId = 0;
    processor.messageTextDom = rendered ? host : null;
    processor.result = 'Existing partial story';
    processor.generator = async function* () { throw new Error('Synthetic stream disconnect'); };
    processor.onFinishStreaming = () => assert.fail('An error must not use successful completion');
    return { context, processor, drafts, host, events, flushed, unlocked: () => unlocked };
}

function replay(f, frames, showThoughts = true) {
    Object.assign(f.context, {
        oai_settings: { show_thoughts: showThoughts },
        chat_completion_sources: { CUSTOM: 'custom' },
        custom_api_formats: { CLAUDE_MESSAGES: 'claude' },
        OPENCODE_API_FORMAT: { CLAUDE_MESSAGES: 'claude', GEMINI: 'gemini' },
        isCustomGeminiGenerateContent: () => false,
    });
    vm.runInContext(replyCode, f.context);
    f.processor.result = '';
    f.processor.onProgressStreaming = async () => {};
    f.processor.generator = async function* () {
        let text = '';
        const state = { reasoning: '', images: [], usage: {} };
        for (const frame of frames) {
            text += f.context.getStreamingReply(frame, state, {
                chatCompletionSource: 'custom', customApiFormat: 'chat-completions',
            });
            Object.assign(state.usage, frame.usage);
            yield { text, toolCalls: [], state };
        }
    };
}

for (const type of ['normal', 'regenerate', 'swipe', 'continue', 'impersonate']) {
    for (const showThoughts of [true, false]) {
        test(`${type}: reasoning-only stop rejects instead of completing, thoughts visible=${showThoughts}`, async () => {
            const f = fixture(type);
            f.processor.continueMessage = type === 'continue' ? 'Previously saved story' : '';
            replay(f, [
                { choices: [{ delta: { reasoning_content: 'Synthetic reasoning summary' } }] },
                { choices: [{ delta: {}, finish_reason: 'stop' }] },
                { choices: [], usage: { completion_tokens_details: { text_tokens: 8519 } } },
            ], showThoughts);
            await assert.rejects(f.processor.generate(), { code: 'generation.empty_output' });
            assert.equal(f.processor.isFinished, false);
            assert.equal(f.processor.isStopped, true);
            assert.equal(f.unlocked(), true);
            assert.deepEqual(f.events, []);
            assert.equal(f.host.innerHTML, 'Existing partial story');
        });
    }
}

test('a terminal usage-only frame does not erase the delivered reply', async () => {
    const f = fixture();
    replay(f, [
        { choices: [{ delta: { reasoning_content: 'Synthetic summary' } }] },
        { choices: [{ delta: { content: 'Actual reply' } }] },
        { choices: [], usage: { completion_tokens: 20 } },
    ]);
    assert.equal(await f.processor.generate(), 'Actual reply');
    assert.equal(f.processor.isFinished, true);
    assert.equal(f.processor.isStopped, false);
});

for (const output of ['tool', 'image', 'whitespace', 'none', 'cancelled']) {
    test(`${output}: an empty text stream respects non-text output and cancellation`, async () => {
        const f = fixture();
        f.processor.result = '';
        f.processor.onProgressStreaming = async () => {};
        f.processor.generator = async function* () {
            if (output === 'none') return;
            if (output === 'cancelled') {
                f.processor.onStopStreaming();
                return;
            }
            yield {
                text: output === 'whitespace' ? ' \n\t' : '',
                toolCalls: output === 'tool' ? [[{ id: 'call-1', function: { name: 'lookup', arguments: '{}' } }]] : [],
                state: { images: output === 'image' ? ['data:image/png;base64,fixture'] : [] },
            };
        };
        if (['whitespace', 'none'].includes(output)) {
            await assert.rejects(f.processor.generate(), { code: 'generation.empty_output' });
            assert.deepEqual(f.events, []);
        } else {
            await f.processor.generate();
            assert.equal(f.processor.isFinished, true);
            assert.equal(f.processor.isStopped, false);
        }
    });
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
