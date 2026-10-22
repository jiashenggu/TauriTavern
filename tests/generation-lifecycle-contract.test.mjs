import test from 'node:test';
import assert from 'node:assert/strict';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';
import ts from 'typescript';
import { assertGenerationOutput } from '../src/scripts/util/generation-lifecycle.js';

const REPO_ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');




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
