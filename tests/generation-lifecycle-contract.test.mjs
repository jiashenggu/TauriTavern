import test from 'node:test';
import assert from 'node:assert/strict';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';
import ts from 'typescript';
import { assertGenerationOutput } from '../src/scripts/util/generation-lifecycle.js';
import { createGenerationIdleGate } from '../src/scripts/util/generation-idle-gate.js';
import { scrollToLatestAssistantMessage, LATEST_ASSISTANT_MESSAGE_SELECTOR } from '../src/scripts/chat-scroll-hotkeys.js';

const REPO_ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');

function completionScrollFixture() {
    const source = readFileSync(path.join(REPO_ROOT, 'src/script.js'), 'utf8');
    const ast = ts.createSourceFile('script.js', source, ts.ScriptTarget.Latest, true, ts.ScriptKind.JS);
    const functions = ['Generate', 'enterGeneration', 'exitGeneration'];
    const code = ast.statements
        .filter(node => ts.isFunctionDeclaration(node) && functions.includes(node.name?.text))
        .map(node => node.getText(ast).replace(/^export /, '')).join('\n');
    const scrolls = [];
    const frames = new Set([0]);
    const context = vm.createContext({
        window: {}, console,
        chat: [{ mes: 'Previous reply', is_user: false, is_system: false, swipe_id: 0 }],
        currentChatId: 'original',
        getCurrentChatId: () => context.currentChatId,
        generationInFlightCount: 0,
        generationHistoryLocator: null,
        generationCompletedMessage: null,
        generationIdleGate: createGenerationIdleGate(),
        getActiveChatSnapshot: () => ({ ref: { kind: 'character', fileName: 'original' } }),
        invoke: async () => {},
        abortController: new AbortController(),
        cleanupGenerationAfterUnhandledError() {},
        GenerateInternal: async () => {},
        requestId: 0,
        cancelAnimationFrame: id => frames.delete(id),
        scrollToLatestAssistantMessage,
        animateChatScrollTop: () => {
            assert.fail('completion must not start an animation that competes with reading');
        },
        setChatScrollTop: top => {
            assert.equal(frames.size, 0, 'queued follow-bottom frame must be cancelled first');
            scrolls.push({ top, text: context.chat.at(-1).mes });
        },
        $: selector => {
            if (selector === '#chat') return { offset: () => ({ top: 100 }), scrollTop: () => 250 };
            assert.equal(selector, LATEST_ASSISTANT_MESSAGE_SELECTOR);
            return { last: () => ({ length: 1, offset: () => ({ top: 600 }) }) };
        },
    });
    vm.runInContext(code, context);
    return { context, scrolls };
}

test('foreground completion navigates once after reply finalization, including regenerate, swipe and continue', async () => {
    for (const type of ['normal', 'regenerate', 'swipe', 'continue']) {
        const { context, scrolls } = completionScrollFixture();
        const rendered = Promise.withResolvers();
        const finish = Promise.withResolvers();
        context.GenerateInternal = async () => {
            if (type === 'continue') context.chat.at(-1).mes += ' continued';
            else if (type === 'swipe') context.chat.at(-1).swipe_id++;
            else context.chat.push({ mes: 'New reply', is_user: false, is_system: false });
            rendered.resolve();
            await finish.promise;
            return 'provider-result';
        };
        const generation = context.Generate(type);
        await rendered.promise;
        assert.equal(scrolls.length, 0, 'rendering alone must not move the viewport');
        finish.resolve();
        assert.equal(await generation, 'provider-result');
        assert.deepEqual(scrolls, [{ top: 750, text: context.chat.at(-1).mes }], type);
        await context.generationIdleGate.wait();
    }
});

test('nested tool rounds and automatic continuation finish before the single completion scroll', async () => {
    const { context, scrolls } = completionScrollFixture();
    const continuing = Promise.withResolvers();
    const finish = Promise.withResolvers();
    let continuation;
    context.GenerateInternal = async (type, options) => {
        if (!options.depth) return context.Generate('normal', { depth: 1 });
        if (type === 'normal') {
            context.chat.push({ mes: 'First part', is_user: false, is_system: false });
            continuation = context.Generate('continue', { depth: 2 });
            await continuing.promise;
            return 'first-part';
        }
        continuing.resolve();
        await finish.promise;
        context.chat.at(-1).mes += ' and final part';
        return 'final-part';
    };
    await context.Generate('normal');
    assert.equal(scrolls.length, 0, 'the detached continuation is still generating');
    finish.resolve();
    await continuation;
    assert.deepEqual(scrolls, [{ top: 750, text: 'First part and final part' }]);
});

test('background, cancelled, failed and unchanged generations leave the viewport alone', async () => {
    for (const scenario of ['quiet', 'dry-run', 'impersonate', 'unchanged', 'user-only', 'system-only', 'cancelled', 'failed', 'chat-changed']) {
        const { context, scrolls } = completionScrollFixture();
        context.GenerateInternal = async () => {
            if (scenario === 'unchanged') return;
            context.chat.push({ mes: 'New content', is_user: scenario === 'user-only', is_system: scenario === 'system-only' });
            if (scenario === 'cancelled') context.abortController.abort();
            if (scenario === 'failed') throw new Error('generation failed');
            if (scenario === 'chat-changed') context.currentChatId = 'other';
        };
        const generation = context.Generate(
            ['quiet', 'impersonate'].includes(scenario) ? scenario : 'normal', {}, scenario === 'dry-run',
        );
        if (scenario === 'failed') await assert.rejects(generation, /generation failed/);
        else await generation;
        assert.equal(scrolls.length, 0, scenario);
        await context.generationIdleGate.wait();
    }
});

test('a pending foreground scroll survives quiet work but never follows a switch to another chat', async () => {
    for (const switchChat of [false, true]) {
        const { context, scrolls } = completionScrollFixture();
        const started = Promise.withResolvers();
        const finish = Promise.withResolvers();
        context.GenerateInternal = async type => {
            if (type === 'quiet') {
                started.resolve();
                await finish.promise;
            } else {
                context.chat.push({ mes: 'New reply', is_user: false, is_system: false });
            }
        };
        const background = context.Generate('quiet');
        await started.promise;
        await context.Generate('normal');
        assert.equal(scrolls.length, 0);
        if (switchChat) {
            context.currentChatId = 'other';
            context.chat = [{ mes: 'Another chat', is_user: false, is_system: false }];
        }
        finish.resolve();
        await background;
        assert.equal(scrolls.length, switchChat ? 0 : 1);
    }
});



test('Unhandled error cleanup only targets foreground UI lifecycle leaks', async () => {
    const { shouldUnblockGenerationAfterUnhandledError } = await import('../src/scripts/util/generation-lifecycle.js');

    assert.equal(shouldUnblockGenerationAfterUnhandledError({
        dryRun: true,
        isSendPress: true,
        isBodyGenerating: true,
        isGroupGenerating: false,
    }), false);
    assert.equal(shouldUnblockGenerationAfterUnhandledError({
        dryRun: false,
        isSendPress: true,
        isBodyGenerating: false,
        isGroupGenerating: true,
    }), false);
    assert.equal(shouldUnblockGenerationAfterUnhandledError({
        dryRun: false,
        isSendPress: true,
        isBodyGenerating: false,
        isGroupGenerating: false,
    }), true);
    assert.equal(shouldUnblockGenerationAfterUnhandledError({
        dryRun: false,
        isSendPress: false,
        isBodyGenerating: true,
        isGroupGenerating: false,
    }), true);
    assert.equal(shouldUnblockGenerationAfterUnhandledError({
        dryRun: false,
        isSendPress: false,
        isBodyGenerating: false,
        isGroupGenerating: false,
    }), false);
});

test('tool-only Assistant messages do not emit legacy character events', async () => {
    const { shouldEmitCharacterMessageEvents } = await import('../src/scripts/util/generation-lifecycle.js');
    const message = (mes = '', extra = {}) => ({ mes, extra });

    assert.equal(shouldEmitCharacterMessageEvents(message(), false), true);
    assert.equal(shouldEmitCharacterMessageEvents(message(), true), false);
    assert.equal(shouldEmitCharacterMessageEvents(message(' ... '), true), false);
    assert.equal(shouldEmitCharacterMessageEvents(message('Calling a tool'), true), true);
    assert.equal(shouldEmitCharacterMessageEvents(message('', { reasoning: 'Thinking' }), true), true);
    assert.equal(shouldEmitCharacterMessageEvents(message('', { media: [{ url: 'result.png' }] }), true), true);
});

test('non-streaming empty replies reject before saving or appending the old continuation', async () => {
    const source = readFileSync(path.join(REPO_ROOT, 'src/script.js'), 'utf8');
    const ast = ts.createSourceFile('script.js', source, ts.ScriptTarget.Latest, true, ts.ScriptKind.JS);
    const generate = ast.statements.find(node => ts.isFunctionDeclaration(node) && node.name.text === 'GenerateInternal');
    const functions = new Map();
    function visit(node) {
        if (ts.isFunctionDeclaration(node) && node.name) functions.set(node.name.text, node.getText(ast));
        ts.forEachChild(node, visit);
    }
    visit(generate);
    const extract = ast.statements.find(node => ts.isFunctionDeclaration(node) && node.name.text === 'extractMessageFromData');
    for (const type of ['normal', 'regenerate', 'swipe', 'continue', 'quiet', 'impersonate']) {
        const alerts = [];
        let unlocked = 0;
        const saved = [{ mes: 'Previous reply', swipes: ['Previous reply'], extra: { preserved: true } }];
        const context = vm.createContext({
            assertGenerationOutput, type, quietToolRequest: null, jsonSchema: null,
            main_api: 'openai', canPerformToolCalls: false,
            isContinue: type === 'continue', isImpersonate: type === 'impersonate',
            continue_mag: 'Previous reply', chat: saved,
            extractTitleFromData: () => '',
            extractReasoningFromData: () => 'Synthetic summary',
            extractImagesFromData: () => [],
            extractReasoningSignatureFromData: () => null,
            getPromptCacheUsage: () => null,
            saveReply: () => assert.fail('An empty response must not save a reply'),
            saveChatConditional: () => assert.fail('An empty response must not commit success'),
            playMessageSound: () => assert.fail('An empty response must not play the success sound'),
            triggerAutoContinue: () => assert.fail('An empty response must not retry automatically'),
            unblockGeneration: () => unlocked++,
            t: strings => strings.join(''),
            toastr: { error: (...args) => alerts.push(args) },
            console: { log() {} },
            streamingProcessor: null,
        });
        vm.runInContext(extract.getText(ast).replace(/^export /, '') + '\n'
            + functions.get('onSuccess') + '\n' + functions.get('onError'), context);
        const response = {
            choices: [{ index: 0, message: { content: '', reasoning_content: 'Synthetic summary' }, finish_reason: 'stop' }],
            usage: { completion_tokens_details: { text_tokens: 8519 } },
        };
        await assert.rejects(Promise.resolve(response).then(context.onSuccess).catch(context.onError),
            { code: 'generation.empty_output' }, type);
        assert.equal(alerts.length, 1, type);
        assert.equal(unlocked, 1, type);
        assert.deepEqual(saved, [{ mes: 'Previous reply', swipes: ['Previous reply'], extra: { preserved: true } }]);
    }
});
