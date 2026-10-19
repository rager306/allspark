
const extract = require('/tmp/cc_verify/cmp.cjs');
eval(require('fs').readFileSync('/tmp/cc_verify/parse.js','utf8'));
const lines = ['- Rule Confidence: 0.9','- rule. confidence: 0.9','- Rule: Confidence: 0.9','- Rule, confidence: 0.9'];
for (const l of lines) {
  console.log(JSON.stringify(l), 'extract=', JSON.stringify(extract.extractLearningWithConfidence({line:l})), ' parseLearning=', JSON.stringify(parseLearning({line:l})));
}
