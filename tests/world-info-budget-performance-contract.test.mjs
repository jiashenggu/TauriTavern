import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';

const worldInfoSource = await readFile(new URL('../src/scripts/world-info.js', import.meta.url), 'utf8');

test('world-info budget counting reuses upstream batched prefixes instead of rescanning each candidate', () => {
    assert.match(worldInfoSource, /const textToScanTokens = await getTokenCountAsync\(allActivatedText\)/);
    assert.match(worldInfoSource, /getTokenPrefixCountsAsync\(batchBaseContent, batchSuffixes, undefined, remainingBudget\)/);
    assert.match(worldInfoSource, /prefetchedTokenCounts\.has\(entry\)\s*\? prefetchedTokenCounts\.get\(entry\)/);
    assert.match(worldInfoSource, /if \(!entry\.ignoreBudget && \(textToScanTokens \+ newContentTokens\) >= budget\)/);
});
