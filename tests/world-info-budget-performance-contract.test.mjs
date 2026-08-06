import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';

const worldInfoSource = await readFile(new URL('../src/scripts/world-info.js', import.meta.url), 'utf8');

test('world-info budget counting tokenizes each candidate instead of each growing prefix', () => {
    assert.match(worldInfoSource, /let newContentTokens = await getTokenCountAsync\(allActivatedText\)/);
    assert.match(worldInfoSource, /newContentTokens \+= await getTokenCountAsync\(entryContent\)/);
    assert.match(worldInfoSource, /if \(!entry\.ignoreBudget && newContentTokens >= budget\)/);
    assert.doesNotMatch(worldInfoSource, /getTokenCountAsync\(newContent\)/);
});
