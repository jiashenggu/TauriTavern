import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';
import ts from 'typescript';
import * as injections from '../src/scripts/prompt-injections.js';
import * as orderUtils from '../src/scripts/prompt-manager-order-utils.js';
import { exportZhaoweiPreset } from '../scripts/export-zhaowei-preset.mjs';
import { createLatestTaskScheduler } from '../src/scripts/util/latest-task-scheduler.js';
import { projectToolTurns } from '../src/scripts/tauritavern/tool-turn-projection.js';
import { hasClaudeToolUse } from '../src/scripts/tauritavern/claude-native-stream.js';

const raw = readFileSync(new URL('../docs/presets/照微.json', import.meta.url), 'utf8');
const preset = JSON.parse(raw);
const styleId = 'consistent-style';
const prompt = id => preset.prompts.find(p => p.identifier === id);
const plain = value => JSON.parse(JSON.stringify(value));

function declarations(file, constants, seen = new Set()) {
    const source = readFileSync(new URL('../' + file, import.meta.url), 'utf8');
    const ast = ts.createSourceFile(file, source, ts.ScriptTarget.Latest, true, ts.ScriptKind.JS);
    return ast.statements.filter(node => ts.isFunctionDeclaration(node) || ts.isClassDeclaration(node)
        || ts.isVariableStatement(node) && (!constants || node.declarationList.declarations.some(d => constants.has(d.name.getText(ast)))))
        .filter(node => {
            const key = node.name?.getText(ast) ?? node.declarationList.declarations[0].name.getText(ast);
            if (seen.has(key)) return false;
            seen.add(key);
            return true;
        }).map(node => node.getText(ast).replace(/^export\s+/, '')).join('\n');
}

const constants = new Set(['DEFAULT_DEPTH', 'DEFAULT_ORDER', 'AGENT_SYSTEM_PROMPT_IDENTIFIER',
    'AGENT_RESULTS_PROMPT_IDENTIFIER', 'AGENT_TASK_PROMPT_IDENTIFIER', 'AGENT_PROMPT_ROLES',
    'AGENT_ONLY_PROMPT_IDENTIFIERS', 'chatCompletionDefaultPrompts', 'promptManagerDefaultPromptOrders',
    'promptManagerDefaultPromptOrder']);
const seen = new Set();
const assemblyCode = ['src/scripts/PromptManager.js', 'src/scripts/openai.js']
    .map(file => declarations(file, constants, seen)).join('\n');

function assemblyFixture(settings = preset) {
    const errors = [];
    const context = vm.createContext({
        console: { debug() {}, log: (...args) => args.filter(arg => arg?.stack || /^\[ChatCompletion\] \w*Error:/.test(String(arg)))
            .forEach(arg => errors.push(String(arg?.stack ?? arg))),
            warn: (...args) => errors.push(args.join(' ')), error: (...args) => errors.push(args.join(' ')) },
        structuredClone, setTimeout, clearTimeout, performance, ...injections, ...orderUtils, createLatestTaskScheduler, projectToolTurns, hasClaudeToolUse,
        power_user: { console_log_prompts: true, pin_examples: false, persona_description_position: 0 },
        t: parts => Array.isArray(parts) ? parts.join('') : parts,
        debounce: f => f, debounce_timeout: { short: 0 }, getSortableDelay: () => 0,
        selected_group: null, default_main_prompt: '', default_nsfw_prompt: '', default_jailbreak_prompt: '', default_enhance_definitions_prompt: '',
        persona_description_positions: { IN_PROMPT: 0 }, extension_prompt_types: { BEFORE_PROMPT: 0, IN_PROMPT: 2, IN_CHAT: 1 },
        extension_prompt_roles: { SYSTEM: 0, USER: 1, ASSISTANT: 2 }, MAX_INJECTION_DEPTH: 10000,
        getExtensionPromptMaxDepth: () => 10000,
        character_names_behavior: { NONE: 0, DEFAULT: 1, CONTENT: 2, COMPLETION: 3 },
        chat_completion_sources: { CUSTOM: 'custom', CLAUDE: 'claude' }, custom_api_formats: { GEMINI_INTERACTIONS: 'gemini_interactions' },
        interleaved_reasoning_providers: [], tool_reasoning_modes: { DISABLED: 'disabled' },
        ToolManager: { canPerformToolCalls: () => false, isToolCallingSupported: () => false },
        countTokensOpenAIAsync: async messages => Math.max(1, Math.ceil(JSON.stringify(messages).length / 4)),
        uuidv4: () => 'fixture-id',
    });
    vm.runInContext(assemblyCode, context);
    vm.runInContext(`
        substitutePromptParams = content => String(content ?? '').replace(/\{\{(char|user|personality|scenario)\}\}/g,
            (_, key) => ({ char: 'TEST_CHARACTER', user: 'TEST_PLAYER', personality: 'MARK_PERSONALITY', scenario: 'MARK_SCENARIO' })[key]);
        formatWorldInfo = value => String(value ?? '');
        isImageInliningSupported = () => false;
        isVideoInliningSupported = () => false;
        isAudioInliningSupported = () => false;
        isReasoningSignatureSupported = () => false;
        createGenerationParameters = async (settings, model, type, messages) => ({ generate_data: { messages, model } });
    `, context);
    const input = {
        settings: { openai_max_context: 100000, openai_max_tokens: 1000, ...structuredClone(settings), chat_completion_source: 'custom' },
        model: 'test-model', agentMode: false,
        macroContext: { character: { persona: 'MARK_PERSONA', personaPosition: 0 } },
        promptInputs: {
            name2: 'TEST_CHARACTER', charDescription: 'MARK_DESCRIPTION', charPersonality: 'MARK_PERSONALITY', scenario: 'MARK_SCENARIO',
            worldInfoBefore: 'MARK_WORLD_BEFORE', worldInfoAfter: 'MARK_WORLD_AFTER', extensionPrompts: {},
            messageExamples: [[{ name: 'example_user', content: 'MARK_EXAMPLE_USER' }, { name: 'example_assistant', content: 'MARK_EXAMPLE_ASSISTANT' }]],
            messages: [{ role: 'user', content: 'HISTORY_NEW' }, { role: 'assistant', content: 'HISTORY_MIDDLE' }, { role: 'user', content: 'HISTORY_OLD' }],
        },
    };
    return { input, run: async () => {
        const result = await context.assembleOpenAIChatCompletionPrompt(input);
        assert.deepEqual(errors, []);
        return plain(result);
    } };
}

function regexFixture() {
    const context = vm.createContext({
        console, characters: [], this_chid: undefined,
        extension_settings: { disabledExtensions: [], regex: [], preset_allowed_regex: { openai: ['照微'] } },
        getPresetManager: () => ({ apiId: 'openai', getSelectedPresetName: () => '照微', readPresetExtensionField: () => preset.extensions.regex_scripts }),
        substituteParams: value => value, substituteParamsExtended: value => value,
        regexFromString: value => {
            const end = value.lastIndexOf('/');
            return new RegExp(value.slice(1, end), value.slice(end + 1));
        },
    });
    vm.runInContext(declarations('src/scripts/extensions/regex/engine.js'), context);
    return (text, placement, options) => context.getRegexedString(text, placement, options);
}

test('distributable preset contains only portable fields and has complete unique prompt ordering', () => {
    assert.ok(!raw.startsWith('\uFEFF'));
    assert.deepEqual(exportZhaoweiPreset(preset), preset);
    assert.equal(preset.prompts.length, 24);
    const ids = preset.prompts.map(p => p.identifier);
    assert.equal(new Set(ids).size, ids.length);
    for (const group of preset.prompt_order) {
        assert.deepEqual(group.order.map(p => p.identifier).sort(), [...ids].sort());
        const styleIndex = group.order.findIndex(p => p.identifier === styleId);
        assert.equal(group.order[styleIndex].enabled, false);
        assert.equal(group.order[styleIndex - 1].identifier, 'consistent-prose');
        assert.equal(group.order[styleIndex + 1].identifier, 'consistent-state');
    }
    assert.equal(preset.extensions.regex_scripts.length, 8);
    assert.equal(new Set(preset.extensions.regex_scripts.map(r => r.id)).size, 8);
    for (const rule of preset.extensions.regex_scripts) {
        const end = rule.findRegex.lastIndexOf('/');
        assert.doesNotThrow(() => new RegExp(rule.findRegex.slice(1, end), rule.findRegex.slice(end + 1)));
    }
});

test('export strips synthetic connection, sampling, runtime and unknown metadata without mutating input', () => {
    const input = structuredClone(preset);
    Object.assign(input, { custom_url: 'https://example.invalid', proxy_password: 'fixture-only',
        additional_parameters_by_source: { openai: { include_headers: 'X-Test: fixture' } },
        custom_include_headers: 'X-Test: fixture', top_p: 0.25, openai_max_context: 2000000, future_private_field: 'fixture' });
    input.extensions.unrelated_script = { code: 'fixture' };
    input.prompts[0].private_metadata = 'fixture';
    input.extensions.regex_scripts[0].private_metadata = 'fixture';
    assert.deepEqual(exportZhaoweiPreset(input), preset);
    assert.equal(input.proxy_password, 'fixture-only');
    assert.throws(() => exportZhaoweiPreset({}), /Expected a Chat Completion preset/);
});

test('humanity and style instructions preserve task, knowledge and player boundaries', () => {
    const style = prompt(styleId);
    assert.equal(style.enabled, false);
    assert.equal(style.role, 'system');
    assert.ok(style.content.includes('文风要求：\n\n参考片段：'));
    for (const phrase of ['不继承范文中的人物、事件', '不覆盖玩家权限', '纯元指令、摘要、提取、独立 MVU']) assert.ok(style.content.includes(phrase));
    for (const phrase of ['角色有正在过的生活', '允许局部修复', '不为展示缺点']) assert.ok(prompt('consistent-world').content.includes(phrase));
    for (const phrase of ['对白先回应对方实际说了什么', '不必再由旁白翻译', '不把每轮结尾']) assert.ok(prompt('consistent-prose').content.includes(phrase));
});

test('production regex processing excludes drafts only from assistant history and preserves state blocks', () => {
    const regex = regexFixture();
    const state = '<content>Story</content><UpdateVariable><JSONPatch>[]</JSONPatch></UpdateVariable><!-- state: keep -->';
    const message = '<draft_notes>Working draft</draft_notes>' + state;
    assert.equal(regex(message, 2, { isPrompt: true }), state);
    assert.equal(regex('<draft_notes>Unfinished draft', 2, { isPrompt: true }), '');
    assert.equal(regex(message, 1, { isPrompt: true }), message);
    assert.equal(regex(message, 2, {}), message);
    const display = regex(message, 2, { isMarkdown: true });
    assert.match(display, /<details class="draft-notes">/);
    assert.ok(display.includes('Working draft'));
    assert.ok(display.includes(state));
    assert.match(regex('<draft_notes>Unfinished draft', 2, { isMarkdown: true }), /<details class="draft-notes" open>/);
});

test('production assembly includes active modules and markers once, retains history order and omits disabled style', async () => {
    const { messages } = await assemblyFixture().run();
    const enabled = new Set(preset.prompt_order[0].order.filter(p => p.enabled).map(p => p.identifier));
    for (const p of preset.prompts.filter(p => enabled.has(p.identifier) && !p.marker && p.content)) {
        assert.equal(messages.filter(m => m.content === p.content).length, 1, p.identifier);
    }
    assert.ok(!messages.some(m => m.content === prompt(styleId).content));
    for (const marker of ['MARK_PERSONA', 'MARK_DESCRIPTION', 'MARK_PERSONALITY', 'MARK_SCENARIO', 'MARK_WORLD_BEFORE', 'MARK_WORLD_AFTER', 'MARK_EXAMPLE_USER']) {
        assert.ok(messages.some(m => m.content.includes(marker)), marker);
    }
    assert.deepEqual(messages.filter(m => m.content.startsWith('HISTORY_')).map(m => m.content), ['HISTORY_OLD', 'HISTORY_MIDDLE', 'HISTORY_NEW']);
});

test('enabled style is sent exactly once between prose and state; quiet task retains final position', async () => {
    const enabled = structuredClone(preset);
    enabled.prompts.find(p => p.identifier === styleId).enabled = true;
    enabled.prompt_order[0].order.find(p => p.identifier === styleId).enabled = true;
    enabled.prompts.find(p => p.identifier === styleId).content += '\nSTYLE_SENTINEL: 口语自然。';
    for (const quiet of [false, true]) {
        const f = assemblyFixture(enabled);
        if (quiet) {
            f.input.generationType = 'quiet';
            f.input.promptInputs.quietPrompt = 'Return only {"ok":true}';
        }
        const { messages } = await f.run();
        const index = text => messages.findIndex(m => m.content.includes(text));
        assert.equal(messages.filter(m => m.content.includes('STYLE_SENTINEL')).length, 1);
        assert.ok(index('STYLE_SENTINEL') > index(prompt('consistent-prose').content));
        assert.ok(index('STYLE_SENTINEL') < index(prompt('consistent-state').content));
        if (quiet) assert.deepEqual(messages.at(-1), { role: 'system', content: 'Return only {"ok":true}' });
    }
});

test('production assembly respects disabled modules, extension depth and history budget', async () => {
    const f = assemblyFixture();
    f.input.settings.prompt_order[0].order.find(p => p.identifier === 'consistent-world').enabled = false;
    f.input.promptInputs.extensionPrompts = {
        archive: { value: 'SLOT_ARCHIVE', position: 1, depth: 9999, role: 0 },
        state: { value: 'SLOT_STATE', position: 1, depth: 1, role: 0 },
    };
    const { messages } = await f.run();
    assert.ok(!messages.some(m => m.content === prompt('consistent-world').content));
    const index = text => messages.findIndex(m => m.content.includes(text));
    assert.equal(messages.filter(m => m.content.includes('SLOT_ARCHIVE')).length, 1);
    assert.equal(messages.filter(m => m.content.includes('SLOT_STATE')).length, 1);
    assert.ok(index('SLOT_ARCHIVE') < index('HISTORY_OLD'));
    assert.ok(index('HISTORY_MIDDLE') < index('SLOT_STATE'));
    assert.ok(index('SLOT_STATE') < index('HISTORY_NEW'));
    const budget = assemblyFixture();
    budget.input.settings.openai_max_context = 6000;
    budget.input.promptInputs.messages[2].content += 'x'.repeat(100000);
    const result = await budget.run();
    assert.ok(result.messages.some(m => m.content === 'HISTORY_NEW'));
    assert.ok(!result.messages.some(m => m.content.startsWith('HISTORY_OLD')));
});
