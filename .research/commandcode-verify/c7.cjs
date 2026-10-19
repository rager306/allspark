
const {parseCategories, categorySlug} = require('/tmp/cc_verify/taste.cjs');
const cases = {
 'normal': '# Taste (Continuously Learned by [CommandCode][cmd])\n\n[cmd]: https://commandcode.ai/\n\n# mcp\n- A. Confidence: 0.65\n\n# cli\n- B. Confidence: 0.5\n',
 'no header, starts with bullet': '- Prefers visual HTML reports. Confidence: 0.9\n- x2. Confidence: 0.8\n- x3. Confidence: 0.8\n- x4. Confidence: 0.8\n- x5. Confidence: 0.8\n- x6. Confidence: 0.8\n',
 'heading with slash': '# css/styling\n- A. Confidence: 0.9\n',
 'heading with apostrophe': "# user's workflow\n- A. Confidence: 0.9\n",
 'heading with backtick+parens': "# prefers built-in hooks (`onStop` with {continue:true})\n- A. Confidence: 0.9\n",
 'duplicate slug collision': '# Workflow A\n- A. Confidence: 0.9\n# workflow-a\n- B. Confidence: 0.9\n',
 'see-reference category': '# cli\nSee [cli/taste.md](cli/taste.md)\n',
};
for (const [k,v] of Object.entries(cases)) {
  const c = parseCategories(v);
  console.log('---', k);
  for (const x of c) console.log('   name=', JSON.stringify(x.name), 'slug=', JSON.stringify(categorySlug(x.name)), 'n=', x.learningCount);
  if (c.length===0) console.log('   (no categories)');
}
console.log('--- slug length for long heading');
const long = 'preference-' + 'a'.repeat(280);
const s = categorySlug('preference-' + 'a'.repeat(280));
console.log('   len=', s.length);
