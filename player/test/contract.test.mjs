import { test } from "node:test";
import assert from "node:assert/strict";
import { load, makePage, fixtureHtml, fixtureExpected } from "./helpers.mjs";

test("fixture blocks match the Python contract", async () => {
  const { content } = makePage(fixtureHtml());
  const blocks = load().extractBlocks(content);
  const got = await Promise.all(blocks.map(async (b) => ({ tag: b.tag, text: b.text, hash: await load().hashText(b.text), tokens: b.tokens.length })));
  assert.deepEqual(got, fixtureExpected());
});

test("collapse uses the explicit class", () => {
  const { collapse } = load();
  assert.equal(collapse(" a  b  c\n"), "a b c");
  assert.equal(collapse("a﻿b"), "a﻿b");
});

test("hash is NFC SHA-1 prefix", async () => {
  assert.equal(await load().hashText("Grüezi"), "94a9bb2c");
});
