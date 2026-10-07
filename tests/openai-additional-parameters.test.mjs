import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';
import ts from 'typescript';

const source = readFileSync(new URL('../src/scripts/openai.js', import.meta.url), 'utf8');
const ast = ts.createSourceFile('openai.js', source, ts.ScriptTarget.Latest, true, ts.ScriptKind.JS);
const functions = new Set(['createAdditionalParametersEntry', 'isPlainObject', 'normalizeAdditionalParametersEntry',
    'getAdditionalParametersStore', 'getAdditionalParametersSourceKey', 'getAdditionalParametersForSource',
    'getLegacyAdditionalParameters', 'applyAdditionalParametersToRequest', 'migrateAdditionalParameters',
    'getCustomModelsStore', 'normalizeKnownCustomModelsStore', 'normalizeCustomModelsForSource', 'normalizeCustomModelId',
    'migrateCustomModels', 'migrateChatCompletionSettings', 'onSettingsPresetChange', 'onPresetImportFileChange', 'hasSensitiveFieldValue']);
const constants = new Set(['additional_parameters_migration_version', 'custom_api_formats', 'custom_source_variants',
    'chat_completion_sources', 'settingsToUpdate', 'custom_prompt_post_processing_types', 'character_names_behavior', 'sensitiveFields']);
const code = ast.statements.filter(node => ts.isFunctionDeclaration(node) && functions.has(node.name?.text)
    || ts.isVariableStatement(node) && node.declarationList.declarations.some(d => constants.has(d.name.getText(ast))))
    .map(node => node.getText(ast).replace(/^export\s+/, '')).join('\n');
const context = vm.createContext({});
vm.runInContext(code, context);
const empty = { include_body: '', exclude_body: '', include_headers: '' };
const plain = value => JSON.parse(JSON.stringify(value));

for (const chat_completion_source of ['openai', 'claude', 'deepseek', 'custom']) {
    test(`${chat_completion_source}: new overrides preserve the configured top_p`, () => {
        const settings = { chat_completion_source };
        const request = { top_p: 0.25 };
        context.applyAdditionalParametersToRequest(request, settings);
        assert.equal(request.top_p, 0.25);
        assert.equal(request.custom_exclude_body, '');
        assert.deepEqual(plain(settings.additional_parameters_by_source), {});
        assert.deepEqual(plain(context.getAdditionalParametersForSource(settings)), empty);
    });
}

test('empty legacy overrides stay empty after migration', () => {
    const settings = { chat_completion_source: 'openai', custom_include_body: '', custom_exclude_body: '', custom_include_headers: '' };
    assert.equal(context.migrateAdditionalParameters(settings), true);
    assert.deepEqual(plain(context.getAdditionalParametersForSource(settings)), empty);
    assert.equal(context.migrateAdditionalParameters(settings), false);
});

test('legacy nonempty overrides survive normalization and migration', () => {
    for (const entry of [undefined, {}, { include_body: 'temperature: 0.7' }]) {
        const settings = { chat_completion_source: 'custom', custom_include_body: 'temperature: 0.2',
            custom_exclude_body: '- top_k', custom_include_headers: 'X-Test: fixture',
            additional_parameters_by_source: entry === undefined ? {} : { custom: entry } };
        context.migrateAdditionalParameters(settings);
        assert.deepEqual(plain(context.getAdditionalParametersForSource(settings)), {
            include_body: entry?.include_body ?? 'temperature: 0.2', exclude_body: '- top_k', include_headers: 'X-Test: fixture',
        });
        assert.equal(context.migrateAdditionalParameters(settings), false);
    }
});

test('explicit top_p exclusion remains opt-in and isolated to its source', () => {
    const settings = { chat_completion_source: 'openai', additional_parameters_migration_version: 1,
        additional_parameters_by_source: { openai: { ...empty, exclude_body: '- top_p' } } };
    context.migrateAdditionalParameters(settings);
    assert.equal(context.getAdditionalParametersForSource(settings).exclude_body, '- top_p');
    settings.chat_completion_source = 'claude';
    assert.deepEqual(plain(context.getAdditionalParametersForSource(settings)), empty);
});

test('missing fields normalize neutrally without replacing explicit values', () => {
    assert.deepEqual(plain(context.normalizeAdditionalParametersEntry({ include_headers: 'X-Test: fixture' })), {
        ...empty, include_headers: 'X-Test: fixture',
    });
    assert.throws(() => context.normalizeAdditionalParametersEntry({ exclude_body: [] }), /must be a string/);
});

const portablePreset = JSON.parse(readFileSync(new URL('../docs/presets/照微.json', import.meta.url), 'utf8'));
const portablePresetWithDefaults = { ...portablePreset, claude_fast_mode: false, strip_old_tool_calls: false };

function selectionFixture(preset, bind = true) {
    let completion;
    let saves = 0;
    const settings = {
        preset_settings_openai: 'Previous', bind_preset_to_connection: bind,
        chat_completion_source: 'custom', custom_url: 'https://example.invalid', custom_model: 'fixture-model',
        additional_parameters_by_source: { custom: { include_body: 'fixture: true', exclude_body: '- top_p', include_headers: 'X-Fixture: value' } },
        additional_parameters_migration_version: 1,
        custom_models_by_source: { openai: ['fixture-model'] },
    };
    const original = structuredClone(settings);
    const dom = { find() { return this; }, text: () => '照微', val() { return this; }, prop() { return this; },
        trigger() { return this; }, empty() { return this; } };
    const presets = [structuredClone(preset)];
    const context = vm.createContext({
        structuredClone, oai_settings: settings, openai_settings: presets, openai_setting_names: { '照微': 0 },
        $: selector => selector === '#settings_preset_openai' ? { ...dom, append() { return this; }, trigger(event) {
            if (event === 'change') context.onSettingsPresetChange();
            return this;
        } } : dom,
        event_types: { OAI_PRESET_CHANGED_BEFORE: 'before' },
        eventSource: { emit: event => event === 'before' ? { finally(callback) { completion = callback(); } } : Promise.resolve() },
        saveOpenAIPreset() {}, saveSettingsDebounced() { saves++; }, syncChatCompletionSourceSelector() {}, applyCustomModelOptionsToAllSources() {},
        getChatCompletionModelControl: source => ({ source, supportsCustomModels: true }),
    });
    vm.runInContext(code, context);
    return { context, settings, original, presets, saves: () => saves, settled: () => completion, select: async () => {
        context.onSettingsPresetChange();
        await completion;
    } };
}

function importFixture(bind, { overwrite = true, confirm = true, saveOk = true, removeSensitive = false } = {}) {
    const stale = { ...portablePreset, chat_completion_source: 'custom', custom_url: 'https://stale.example.invalid',
        custom_model: 'stale-model', reverse_proxy: 'https://stale-proxy.example.invalid',
        additional_parameters_by_source: { custom: { ...empty, exclude_body: '- top_k' } },
        additional_parameters_migration_version: 1, custom_models_by_source: { openai: ['stale-model'] },
        temp_openai: 0.1, unknown_old_field: 'stale' };
    const f = selectionFixture(stale, bind);
    const requests = [];
    const errors = [];
    if (!overwrite) {
        f.presets.length = 0;
        delete f.context.openai_setting_names['照微'];
    }
    Object.assign(f.context, {
        console: { log() {}, error: (...args) => errors.push(args) },
        getFileText: async file => file.text,
        callGenericPopup: async () => confirm, POPUP_TYPE: { CONFIRM: 'confirm' },
        POPUP_RESULT: { CANCELLED: 'cancelled', AFFIRMATIVE: 'remove' },
        Popup: { show: { confirm: async () => removeSensitive ? 'remove' : 'keep' } },
        fetch: async (url, request) => {
            assert.equal(url, '/api/presets/save');
            requests.push(JSON.parse(request.body));
            return { ok: saveOk, json: async () => ({ name: '照微' }) };
        },
        getRequestHeaders: () => ({}), document: { createElement: () => ({}) },
        toastr: { error() {} }, t: value => value,
    });
    return { ...f, stale: structuredClone(stale), requests, import: async (preset = portablePreset) => {
        const target = { files: [{ name: '照微.json', text: JSON.stringify(preset) }], value: 'fixture' };
        await f.context.onPresetImportFileChange({ target });
        await f.settled();
        assert.equal(target.value, '');
        for (const [message, error] of errors) {
            assert.equal(message, 'Failed to start embedded Agent asset import prompt for OpenAI preset');
            assert.equal(error.code, 'ERR_VM_DYNAMIC_IMPORT_CALLBACK_MISSING');
        }
    } };
}

for (const bind of [true, false]) {
    for (const overwrite of [false, true]) {
        test(`portable import preserves active settings with overwrite=${overwrite}, binding=${bind}`, async () => {
            const f = importFixture(bind, { overwrite });
            await f.import();
            assert.deepEqual(f.requests[0].preset, portablePreset);
            assert.deepEqual(plain(f.presets[0]), portablePresetWithDefaults);
            await f.select();
            for (const [key, value] of Object.entries(f.original).filter(([key]) => key !== 'preset_settings_openai')) {
                assert.deepEqual(plain(f.settings[key]), value, key);
            }
            for (const key of ['custom_url', 'reverse_proxy', 'unknown_old_field']) assert.equal(Object.hasOwn(f.presets[0], key), false);
        });
    }

    for (const removeSensitive of [false, true]) {
        test(`overwrite applies only approved imported connection fields with binding=${bind}, removeSensitive=${removeSensitive}`, async () => {
            const f = importFixture(bind, { removeSensitive });
            const imported = { ...portablePreset, chat_completion_source: 'custom', custom_url: 'https://imported.example.invalid' };
            await f.import(imported);
            const expected = { ...structuredClone(imported), claude_fast_mode: false, strip_old_tool_calls: false };
            const expectedRequest = structuredClone(imported);
            if (removeSensitive) { delete expected.custom_url; delete expectedRequest.custom_url; }
            assert.deepEqual(f.requests[0].preset, expectedRequest);
            assert.deepEqual(plain(f.presets[0]), expected);
            assert.equal(f.settings.custom_url, bind && !removeSensitive ? imported.custom_url : f.original.custom_url);
            assert.equal(f.settings.custom_model, f.original.custom_model);
            assert.deepEqual(plain(f.settings.additional_parameters_by_source), f.original.additional_parameters_by_source);
            assert.deepEqual(plain(f.settings.custom_models_by_source), f.original.custom_models_by_source);
        });
    }
}

for (const options of [{ confirm: false }, { saveOk: false }]) {
    test(`cancelled or failed overwrite leaves cached and active settings unchanged: ${JSON.stringify(options)}`, async () => {
        const f = importFixture(true, options);
        await f.import();
        assert.deepEqual(plain(f.presets[0]), f.stale);
        assert.deepEqual(plain(f.settings), f.original);
        assert.equal(f.saves(), 0);
        assert.equal(f.requests.length, options.confirm === false ? 0 : 1);
    });
}

for (const bind of [true, false]) {
    test(`portable preset selection preserves connection stores with binding=${bind}, including repeat selection`, async () => {
        const f = selectionFixture(portablePreset, bind);
        for (let i = 0; i < 2; i++) {
            await f.select();
            for (const key of Object.keys(f.original).filter(key => key !== 'preset_settings_openai')) {
                assert.deepEqual(plain(f.settings[key]), f.original[key], key);
            }
            assert.deepEqual(plain(f.settings.prompts), portablePreset.prompts);
            assert.deepEqual(plain(f.settings.prompt_order), portablePreset.prompt_order);
            assert.deepEqual(plain(f.settings.extensions), portablePreset.extensions);
            assert.equal(Object.hasOwn(f.presets[0], 'additional_parameters_by_source'), false);
            assert.equal(Object.hasOwn(f.presets[0], 'custom_models_by_source'), false);
        }
        assert.equal(f.saves(), 2);
    });

    for (const value of [{}, { openai: { ...empty, exclude_body: '- top_k' } }]) {
        test(`explicit request store ${JSON.stringify(value)} is applied only when binding=${bind}`, async () => {
            const f = selectionFixture({ ...portablePreset, additional_parameters_by_source: value }, bind);
            await f.select();
            assert.deepEqual(plain(f.settings.additional_parameters_by_source), bind ? value : f.original.additional_parameters_by_source);
            assert.deepEqual(plain(f.settings.custom_models_by_source), f.original.custom_models_by_source);
        });
    }

    for (const value of [{}, { openai: ['new-fixture-model'] }]) {
        test(`explicit custom model store ${JSON.stringify(value)} is applied only when binding=${bind}`, async () => {
            const f = selectionFixture({ ...portablePreset, custom_models_by_source: value }, bind);
            await f.select();
            assert.deepEqual(plain(f.settings.custom_models_by_source), bind ? value : f.original.custom_models_by_source);
            assert.deepEqual(plain(f.settings.additional_parameters_by_source), f.original.additional_parameters_by_source);
        });
    }

    for (const exclude of ['', '- top_k']) {
        test(`legacy request fields migrate on preset selection with binding=${bind} and exclude=${JSON.stringify(exclude)}`, async () => {
            const f = selectionFixture({ ...portablePreset, chat_completion_source: 'custom',
                custom_include_body: '', custom_exclude_body: exclude, custom_include_headers: '' }, bind);
            await f.select();
            const expected = exclude ? { custom: { ...empty, exclude_body: exclude } } : {};
            assert.deepEqual(plain(f.settings.additional_parameters_by_source), bind ? expected : f.original.additional_parameters_by_source);
            assert.deepEqual(plain(f.settings.custom_models_by_source), f.original.custom_models_by_source);
            await f.select();
            assert.deepEqual(plain(f.settings.additional_parameters_by_source), bind ? expected : f.original.additional_parameters_by_source);
        });
    }
}

test('runtime migration still initializes missing stores and is idempotent', () => {
    const settings = {};
    assert.equal(context.migrateChatCompletionSettings(settings), true);
    assert.deepEqual(plain(settings.additional_parameters_by_source), {});
    assert.deepEqual(plain(settings.custom_models_by_source), {});
    assert.equal(context.migrateChatCompletionSettings(settings), false);
});
