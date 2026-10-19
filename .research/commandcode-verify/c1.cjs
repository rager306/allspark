
const {t} = require('/tmp/cc_verify/t.cjs');
// 1. classic alias rename
t('edit_file', {path:'/a.ts', oldValue:'x', newValue:'y'}, 'aliases path/oldValue/newValue');
// 2. alias + empty-string alias (skippedEmptyAlias path)
t('edit_file', {path:'', oldValue:'x', newValue:'y'}, 'empty path alias -> reject w/ hint');
// 3. conflict: BOTH canonical and alias present
t('edit_file', {file_path:'/a.ts', path:'/b.ts', old_string:'x', new_string:'y'}, 'canonical + alias conflict (which wins?)');
// 4. conflicting alias old + oldValue
t('edit_file', {path:'/a.ts', old:'AAA', oldValue:'BBB', new:'y'}, 'two aliases for old_string');
// 5. grep with query
t('grep', {query:'fn main', directory:'src'}, 'grep query->pattern, directory->path');
// 6. grep -A as string
t('grep', {pattern:'x', '-A':'3'}, 'grep numeric string -A');
// 7. read_file with include (multi) instead of paths
t('read_file', {include:['a.ts','b.ts']}, 'read_file include->paths, file_path satisfied?');
// 8. read_file bare string root
// 9. shell_output action:kill
t('shell_output', {id:'s1', action:'kill'}, 'shell_output action:kill redirect hint');
// 10. todo_write items alias + nested alias content
t('todo_write', {items:[{task:'do x', state:'done', active_form:'doing x'}]}, 'todo_write items->todos, nested aliases');
