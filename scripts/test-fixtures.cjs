// Every fixture is owned explicitly; the journey runner removes its containing run folder.
const fs=require('node:fs'),os=require('node:os'),path=require('node:path');
function create(prefix){
 const base=process.env.SDIR_TEST_FIXTURE_ROOT||os.tmpdir();
 if(process.env.SDIR_TEST_FIXTURE_ROOT){
  const marker=path.join(base,'.sdir-test-run.json');
  if(!path.resolve(base).toLowerCase().startsWith((path.resolve(os.tmpdir())+path.sep).toLowerCase())||!fs.existsSync(marker))throw Error('Unmarked test fixture parent');
 }
 const root=fs.mkdtempSync(path.join(base,prefix));
 fs.writeFileSync(path.join(root,'.sdir-audit-fixture'),'owned');
 if(process.env.SDIR_TEST_FIXTURE_ROOT)fs.appendFileSync(path.join(base,'.sdir-fixtures.jsonl'),JSON.stringify({root,pid:process.pid,script:path.basename(process.argv[1]),created:new Date().toISOString()})+'\n');
 return root;
}
module.exports={create};
