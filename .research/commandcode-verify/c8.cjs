
const {extractLearningWithConfidence, compareLearnings} = require('/tmp/cc_verify/cmp.cjs');
const lines = [
 '- Prefers tabs. Confidence: 0.9',
 '- Prefers tabs Confidence: 0.9',
 '- prefers tabs. Confidence: 0.9',
 '- Use `a.b` then c. Confidence: 0.9',
 '- Rule.  Confidence:   0.90',
 '  - Indented rule. Confidence: 0.8',
 '- Rule with no confidence at all',
 '- Rule. Confidence: 1.5',
 '- Rule. Confidence: .5',
];
for (const l of lines) {
  const e = extractLearningWithConfidence({line:l});
  console.log(JSON.stringify(l), '=>', e?JSON.stringify(e):'null');
}
console.log('--- compare: reworded rule');
const a = '# c\n- Use pnpm. Confidence: 0.7\n- Old rule. Confidence: 0.5\n';
const b = '# c\n- Use pnpm for installs. Confidence: 0.7\n- Old rule. Confidence: 0.5\n';
console.log(JSON.stringify(compareLearnings({oldText:a,newText:b})));
console.log('--- compare: rule DELETED (no removal signal?)');
console.log(JSON.stringify(compareLearnings({oldText:'# c\n- Use pnpm. Confidence: 0.7\n', newText:'# c\n'})));
