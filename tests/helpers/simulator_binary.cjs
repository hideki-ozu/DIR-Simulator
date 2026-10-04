'use strict';
const path=require('node:path');
const {execFileSync}=require('node:child_process');

// DIR_SIMULATOR_BIN uses a prebuilt executable; otherwise CARGO (or PATH cargo)
// builds the debug executable for the current platform.
function simulatorBinary(repository,{env=process.env,platform=process.platform,execute=execFileSync}={}) {
 if(env.DIR_SIMULATOR_BIN)return path.resolve(repository,env.DIR_SIMULATOR_BIN);
 execute(env.CARGO||'cargo',['build','--locked','-p','dir-simulator'],{cwd:repository,stdio:'pipe'});
 return path.join(repository,'target','debug',platform==='win32'?'dir-simulator.exe':'dir-simulator');
}
module.exports={simulatorBinary};
