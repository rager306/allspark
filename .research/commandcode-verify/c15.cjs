
const {parseCategories, categorySlug} = require('/tmp/cc_verify/reorg.cjs');
const HDR = '# Taste (Continuously Learned by [CommandCode][cmd])\n\n[cmd]: https://commandcode.ai/\n';
const body = Array.from({length:6},(_,i)=>`- Rule ${i}. Confidence: 0.9`).join('\n');
console.log('--- header + bullets, NO category heading (what the learner writes if it forgets the category)');
for (const c of parseCategories(HDR+'\n'+body+'\n')) console.log('   name=',JSON.stringify(c.name),'slug=',JSON.stringify(categorySlug(c.name)),'n=',c.learningCount);
console.log('--- same but 5 bullets (under threshold)');
for (const c of parseCategories(HDR+'\n'+Array.from({length:5},(_,i)=>`- Rule ${i}. Confidence: 0.9`).join('\n')+'\n')) console.log('   name=',JSON.stringify(c.name),'n=',c.learningCount);
console.log('--- bullet line AS heading');
for (const c of parseCategories('# - Prefers visual HTML reports. Confidence: 0.9\n'+Array.from({length:5},(_,i)=>`- R${i}. Confidence: 0.9`).join('\n')+'\n')) console.log('   name=',JSON.stringify(c.name),'slug=',JSON.stringify(categorySlug(c.name)),'n=',c.learningCount);
console.log('--- "## " subheading is NOT a category boundary (split only on "# ")');
for (const c of parseCategories('# a\n- r. Confidence: 0.9\n## b\n- r2. Confidence: 0.9\n')) console.log('   name=',JSON.stringify(c.name),'n=',c.learningCount);
