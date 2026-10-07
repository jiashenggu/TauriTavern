import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import vm from 'node:vm';
import ts from 'typescript';

const REPO_ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');

function readProjectFile(relativePath) {
    return readFile(path.join(REPO_ROOT, relativePath), 'utf8');
}

test('chat completion custom models are implemented through the shared model-control contract', async () => {
    const openaiSource = await readProjectFile('src/scripts/openai.js');

    assert.match(openaiSource, /const chatCompletionModelControls = \{/);
    assert.match(openaiSource, /export function getChatCompletionModelControl/);
    assert.match(openaiSource, /\[chat_completion_sources\.MINIMAX\]:\s*\{\s*selector:\s*'#model_minimax_select',\s*settingKey:\s*'minimax_model'[\s\S]*?supportsCustomModels:\s*true\s*\}/);
    assert.match(openaiSource, /\[chat_completion_sources\.WORKERS_AI\]:\s*\{\s*selector:\s*'#model_workers_ai_select',\s*settingKey:\s*'workers_ai_model'[\s\S]*?supportsCustomModels:\s*true\s*\}/);
    assert.match(openaiSource, /custom_models_by_source:\s*\['',\s*'custom_models_by_source',\s*false,\s*true\]/);
    assert.match(openaiSource, /custom_models_by_source:\s*\{\}/);
    assert.match(openaiSource, /function normalizeKnownCustomModelsStore/);
    assert.match(openaiSource, /const control = getChatCompletionModelControl\(source\);\s*if \(!control\?\.supportsCustomModels\) \{\s*continue;\s*\}/);
    const customModelsStoreBody = openaiSource.match(/function getCustomModelsStore[\s\S]*?\n}\n\nfunction normalizeKnownCustomModelsStore/)?.[0] ?? '';
    assert.doesNotMatch(customModelsStoreBody, /normalizeCustomModelsForSource/);
    assert.match(openaiSource, /function applyCustomModelOptionsForSource/);
    assert.match(openaiSource, /function handleCustomModelSelectAction/);
    assert.match(openaiSource, /await handleCustomModelSelectAction\(this,\s*value\)/);
    assert.match(openaiSource, /appendOption\(actionGroup,\s*t`Manage custom models\.\.\.`,\s*manage_custom_chat_completion_models_option\)/);
    assert.match(openaiSource, /onClosing:\s*\(popup\) => \{/);
    assert.match(openaiSource, /return addDraftModel\(\{ selectExisting: true \}\)/);
    assert.match(openaiSource, /modelId === currentModel && isCustomModelValueForSource\(control\.source,\s*modelId\)/);
    assert.match(openaiSource, /Popup\.show\.text\(t`Cannot delete current custom model`,\s*t`Switch to another model before deleting it\.`\)/);
    assert.doesNotMatch(openaiSource, /custom_chat_completion_model_option/);
    assert.doesNotMatch(openaiSource, /t`Custom model\.\.\.`/);
});

test('dynamic model lists preserve explicit custom selections instead of falling back silently', async () => {
    const openaiSource = await readProjectFile('src/scripts/openai.js');

    assert.match(openaiSource, /function chooseModelOrCurrentCustom/);
    assert.match(openaiSource, /setModelSelectValue\(chat_completion_sources\.OPENAI,\s*model\)/);
    assert.match(openaiSource, /setModelSelectValue\(chat_completion_sources\.MISTRALAI,\s*oai_settings\.mistralai_model\)/);
    assert.match(openaiSource, /oai_settings\.workers_ai_model = chooseModelOrCurrentCustom\(\s*chat_completion_sources\.WORKERS_AI,/);
    assert.match(openaiSource, /setModelSelectValue\(chat_completion_sources\.WORKERS_AI,\s*oai_settings\.workers_ai_model\)/);
    assert.match(openaiSource, /setModelSelectValue\(chat_completion_sources\.COMETAPI,\s*oai_settings\.cometapi_model\)/);
    assert.match(openaiSource, /!hasModelsLoaded && !isCustomModelValueForSource\(chat_completion_sources\.OPENROUTER,\s*value\)/);
    assert.match(openaiSource, /!hasModelsLoaded && !isCustomModelValueForSource\(chat_completion_sources\.WORKERS_AI,\s*value\)/);
});

test('/model command uses the chat completion registry and excludes select action sentinels', async () => {
    const slashCommandsSource = await readProjectFile('src/scripts/slash-commands.js');

    assert.match(slashCommandsSource, /getChatCompletionModelControl/);
    assert.match(slashCommandsSource, /isCustomModelActionValue/);
    assert.match(slashCommandsSource, /addAndSelectCustomModelForSource/);
    assert.match(slashCommandsSource, /\.filter\(x => !isCustomModelActionValue\(x\.value\)\)/);
    assert.match(slashCommandsSource, /supportsCustomModels && customModelSource[\s\S]*addAndSelectCustomModelForSource\(customModelSource,\s*model\)/);
    assert.doesNotMatch(slashCommandsSource, /\{\s*id:\s*'model_openai_select',\s*api:\s*'openai'/);
});

const source = await readProjectFile('src/scripts/openai.js');
const ast = ts.createSourceFile('openai.js', source, ts.ScriptTarget.Latest, true, ts.ScriptKind.JS);
const names = new Set(['applyCustomModelOptionsForSource', 'removeCustomModelOptionsForSource', 'appendOption',
    'getSourceModelSelect', 'getChatCompletionModelControl', 'getCustomModelsForSource', 'getCustomModelsStore',
    'isPlainObject', 'normalizeCustomModelsForSource', 'normalizeCustomModelId', 'isCustomModelActionValue',
    'chat_completion_sources', 'chatCompletionModelControls', 'custom_model_option_values', 'manage_custom_chat_completion_models_option']);
const modelCode = ast.statements.filter(node => names.has(node.name?.text)
    || ts.isVariableStatement(node) && node.declarationList.declarations.some(d => names.has(d.name.getText(ast))))
    .map(node => node.getText(ast).replace(/^export /, '')).join('\n');

function modelSelectFixture(source, currentModel, customModels) {
    class Option {
        constructor(text, value) { this.text = text; this.value = value; }
    }
    class Select {
        children = [new Option('Built-in A', 'builtin-a'), new Option('Built-in B', 'builtin-b')];
        selected = this.children[0];
        get options() { return this.children.flatMap(child => child.options ?? [child]); }
        get value() { return this.selected?.value ?? ''; }
        set value(value) { this.selected = this.options.find(option => option.value === value); }
        append(group) {
            this.children.push(group);
            group.remove = () => {
                this.children.splice(this.children.indexOf(group), 1);
                if (!this.options.includes(this.selected)) this.selected = this.options[0];
            };
        }
        querySelectorAll(selector) {
            assert.equal(selector, '[data-tauritavern-custom-models="true"]');
            return this.children.filter(child => child.dataset?.tauritavernCustomModels === 'true');
        }
    }
    const select = new Select();
    const settings = { [`${source}_model`]: currentModel, custom_models_by_source: { [source]: customModels } };
    const context = vm.createContext({
        oai_settings: settings, Option, HTMLSelectElement: Select, t: parts => parts.join(''),
        document: {
            querySelector: () => select,
            createElement(tag) {
                assert.equal(tag, 'optgroup');
                return { dataset: {}, options: [], append(option) { this.options.push(option); } };
            },
        },
    });
    vm.runInContext(modelCode, context);
    return { select, settings, rebuild: () => context.applyCustomModelOptionsForSource(source) };
}

for (const source of ['openai', 'claude', 'deepseek']) {
    for (const current of ['custom-b', 'legacy-custom', 'builtin-b']) {
        test(`${source}: rebuilding model options retains ${current} across repeated refreshes`, () => {
            const f = modelSelectFixture(source, current, ['custom-a', 'custom-b']);
            f.rebuild();
            f.select.value = current;
            const original = structuredClone(f.settings);
            for (let i = 0; i < 3; i++) {
                f.rebuild();
                assert.equal(f.select.value, current);
                assert.equal(f.select.options.filter(option => option.value === current).length, 1);
                assert.deepEqual(structuredClone(f.settings), original);
            }
        });
    }

    test(`${source}: an explicitly changed model setting wins over the old DOM selection`, () => {
        const f = modelSelectFixture(source, 'custom-b', ['custom-a', 'custom-b']);
        f.rebuild();
        f.select.value = 'custom-b';
        for (const next of ['builtin-b', 'new-custom', 'custom-a']) {
            f.settings[`${source}_model`] = next;
            f.rebuild();
            assert.equal(f.select.value, next);
        }
    });
}

test('model option refresh without a configured model retains the native default', () => {
    const f = modelSelectFixture('openai', '', ['custom-a']);
    f.rebuild();
    assert.equal(f.select.value, 'builtin-a');
    assert.equal(f.settings.openai_model, '');
});
