
const {validateTasteFile} = require('/tmp/cc_verify/lint.cjs');
const HDR='# Taste (Continuously Learned by [CommandCode][cmd])\n\n[cmd]: https://commandcode.ai/\n';
const cases={
 good: HDR+'\n# workflow\n- Always run tests. Confidence: 0.9\n',
 'no confidence': HDR+'\n# workflow\n- Always run tests.\n',
 'conf out of range': HDR+'\n# workflow\n- Always run tests. Confidence: 1.5\n',
 'learning before any category': HDR+'\n- Orphan rule. Confidence: 0.9\n',
 'empty category': HDR+'\n# workflow\n',
 'multi-line learning continuation': HDR+'\n# workflow\n- Long rule that\n  continues here. Confidence: 0.9\n',
 'lowercase confidence': HDR+'\n# workflow\n- Always run tests. confidence: 0.9\n',
 'empty file': '',
};
for (const [k,v] of Object.entries(cases)) {
  const r=validateTasteFile({content:v,filePath:'taste.md'});
  console.log('---',k,'valid=',r.valid);
  for (const i of r.issues) console.log('    ', i.severity, 'L'+i.line+':', i.message);
}
