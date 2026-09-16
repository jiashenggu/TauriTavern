import { readFile, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { pathToFileURL } from 'node:url';

const promptFields = ['identifier', 'name', 'content', 'role', 'enabled', 'system_prompt', 'marker',
    'forbid_overrides', 'injection_position', 'injection_depth', 'injection_order', 'injection_trigger'];
const regexFields = ['id', 'scriptName', 'findRegex', 'replaceString', 'trimStrings', 'placement',
    'disabled', 'markdownOnly', 'promptOnly', 'runOnEdit', 'substituteRegex', 'minDepth', 'maxDepth'];
const behaviorFields = ['new_chat_prompt', 'new_group_chat_prompt', 'new_example_chat_prompt',
    'continue_nudge_prompt', 'continue_prefill', 'continue_postfix', 'group_nudge_prompt',
    'impersonation_prompt', 'assistant_prefill', 'assistant_impersonation', 'names_behavior',
    'scenario_format', 'personality_format', 'wi_format', 'send_if_empty'];
const select = (value, keys) => Object.fromEntries(keys.filter(key => Object.hasOwn(value, key)).map(key => [key, value[key]]));

export function exportZhaoweiPreset(preset) {
    if (!Array.isArray(preset.prompts) || !Array.isArray(preset.prompt_order)
        || !Array.isArray(preset.extensions?.regex_scripts)) {
        throw new TypeError('Expected a Chat Completion preset with prompts, prompt_order and regex_scripts');
    }
    return structuredClone({
        ...select(preset, behaviorFields),
        prompts: preset.prompts.map(prompt => select(prompt, promptFields)),
        prompt_order: preset.prompt_order.map(group => ({
            character_id: group.character_id,
            order: group.order.map(entry => select(entry, ['identifier', 'enabled'])),
        })),
        extensions: { regex_scripts: preset.extensions.regex_scripts.map(rule => select(rule, regexFields)) },
    });
}

if (process.argv[1] && import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href) {
    const [input, output] = process.argv.slice(2);
    if (!input || !output || process.argv.length !== 4) {
        throw new Error('Usage: node scripts/export-zhaowei-preset.mjs <input-preset.json> <new-output.json>');
    }
    const preset = exportZhaoweiPreset(JSON.parse(await readFile(input, 'utf8')));
    await writeFile(output, JSON.stringify(preset, null, 2) + '\n', { encoding: 'utf8', flag: 'wx' });
    console.log('Exported prompts and regex rules without connection, sampling or runtime settings.');
}
