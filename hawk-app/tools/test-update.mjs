// hawk-update（Windows 更新辅助程序）端到端验证：npm run test:update
// 用真实 hawk-update.exe 走完整安装链路：等旧进程退出 → 解压 → 覆盖 → 拉起 → 清理。
// 覆盖三类分支：等待 hawk.exe 进程退出、非 hawk.exe 进程跳过等待（PID 复用守卫）、坏包失败落日志。
// 测试内「旧进程」用真进程模拟：powershell Start-Sleep（ hawk.exe 名）与 cmd ping（非 hawk.exe 名）。
import { spawn, spawnSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const repo = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..', '..');
const updaterExe = path.join(repo, 'hawk-update', 'target', 'x86_64-pc-windows-msvc', 'release', 'hawk-update.exe');
if (!fs.existsSync(updaterExe)) {
  console.error('未找到 hawk-update.exe，先在 hawk-update/ 下 cargo build --release');
  process.exit(1);
}

const work = path.join(repo, 'hawk-app', 'tools', '.test-update');
fs.rmSync(work, { recursive: true, force: true });

/** 造假应用目录（旧内容）与更新 zip（新内容，布局同 electron-builder win target：hawk.exe 在根） */
function stage(name, version) {
  const appDir = path.join(work, name, 'app');
  fs.mkdirSync(path.join(appDir, 'resources', 'hawk-daemon'), { recursive: true });
  fs.writeFileSync(path.join(appDir, 'hawk.exe'), `${version}-exe`);
  fs.writeFileSync(path.join(appDir, 'resources', 'app.asar'), `${version}-asar`);
  fs.writeFileSync(path.join(appDir, 'resources', 'hawk-daemon', 'hawk-daemon.exe'), `${version}-daemon`);

  const staging = path.join(work, name, 'staging');
  fs.mkdirSync(path.join(staging, 'resources', 'hawk-daemon'), { recursive: true });
  // hawk.exe 用真可执行文件（hostname.exe：spawn 后即退出）：安装链路最后一步是拉起新实例，
  // 假文本文件会在这一步失败；断言用「与 hostname.exe 字节一致」验证版本更替
  fs.copyFileSync('C:\\Windows\\System32\\hostname.exe', path.join(staging, 'hawk.exe'));
  fs.writeFileSync(path.join(staging, 'resources', 'app.asar'), `v2-asar`);
  fs.writeFileSync(path.join(staging, 'resources', 'hawk-daemon', 'hawk-daemon.exe'), `v2-daemon`);
  fs.writeFileSync(path.join(staging, 'new-file.txt'), 'v2-added');

  const zip = path.join(work, name, 'hawk-windows-x64.zip');
  // Windows 内置 bsdtar 按 .zip 后缀选 zip 格式（deflate），与 electron-builder 产物同格式；
  // 绝对路径：避开 PATH 里 MSYS tar 把 D: 盘符当远程主机的问题
  const r = spawnSync('C:\\Windows\\System32\\tar.exe', ['-a', '-cf', zip, '-C', staging, 'hawk.exe', 'resources', 'new-file.txt']);
  if (r.status !== 0) throw new Error(`tar 造 zip 失败：${r.stderr}`);
  return { appDir, zip };
}

let failures = 0;
const assert = (cond, msg) => {
  console.log(`${cond ? 'ok' : 'FAIL'}: ${msg}`);
  if (!cond) failures++;
};

/** 跑一次安装，返回 { code, elapsedMs } */
function runInstall(zip, appDir, oldProcess) {
  const start = Date.now();
  const r = spawnSync(updaterExe, ['--pid', String(oldProcess ? oldProcess.pid : 0), '--zip', zip, '--app', appDir], { timeout: 60_000 });
  return { code: r.status, elapsedMs: Date.now() - start };
}

// ---- 用例 1：旧进程名是 cmd.exe（非 hawk.exe）→ PID 复用守卫应跳过等待，秒级完成 ----
{
  const { appDir, zip } = stage('guard', 'v1');
  const sleepProc = spawn('powershell.exe', ['-NoProfile', '-Command', 'Start-Sleep -Seconds 6'], { stdio: 'ignore' });
  await new Promise((res) => setTimeout(res, 500)); // 等 sleepProc 起来
  const { code, elapsedMs } = runInstall(zip, appDir, sleepProc);
  sleepProc.kill();
  assert(code === 0, `守卫分支：安装成功（exit=${code}）`);
  assert(elapsedMs < 5000, `守卫分支：非 hawk.exe 进程跳过等待（${elapsedMs}ms）`);
}

// ---- 用例 2：旧进程是名为 hawk.exe 的真进程 → 等它退出后才继续 ----
{
  const { appDir, zip } = stage('wait', 'v1');
  // 复制 cmd 为 hawk.exe：进程名即 hawk.exe，2 秒后自行退出
  const fakeExe = path.join(work, 'wait', 'hawk.exe');
  fs.copyFileSync('C:\\Windows\\System32\\cmd.exe', fakeExe);
  const oldProc = spawn(fakeExe, ['/c', 'ping -n 3 127.0.0.1 > nul'], { stdio: 'ignore' });
  await new Promise((res) => setTimeout(res, 500));
  const { code, elapsedMs } = runInstall(zip, appDir, oldProc);
  assert(code === 0, `等待分支：安装成功（exit=${code}）`);
  assert(elapsedMs >= 1000, `等待分支：等到了 hawk.exe 进程退出（${elapsedMs}ms）`);
}

// ---- 用例 3：覆盖正确性（文件/嵌套目录/新增文件）与清理 ----
{
  const { appDir, zip } = stage('clean', 'v1');
  const { code } = runInstall(zip, appDir, null);
  assert(code === 0, `覆盖：安装成功`);
  assert(fs.readFileSync(path.join(appDir, 'resources', 'app.asar'), 'utf8') === 'v2-asar', '覆盖：resources/app.asar 已更新');
  assert(fs.readFileSync(path.join(appDir, 'resources', 'hawk-daemon', 'hawk-daemon.exe'), 'utf8') === 'v2-daemon', '覆盖：嵌套目录已更新');
  assert(
    fs.readFileSync(path.join(appDir, 'hawk.exe')).equals(fs.readFileSync('C:\\Windows\\System32\\hostname.exe')),
    '覆盖：hawk.exe 已更新（新实例可执行）',
  );
  assert(fs.readFileSync(path.join(appDir, 'new-file.txt'), 'utf8') === 'v2-added', '覆盖：新增文件已就位');
  assert(!fs.existsSync(path.join(path.dirname(zip), 'extract')), '清理：extract 已删除');
  assert(!fs.existsSync(zip), '清理：zip 已删除');
  assert(fs.existsSync(path.join(path.dirname(zip), 'install.log')), '诊断：install.log 已落盘');
}

// ---- 用例 4：坏 zip → 非零退出 + 日志记录失败原因 ----
{
  const dir = path.join(work, 'badzip');
  fs.mkdirSync(dir, { recursive: true });
  const zip = path.join(dir, 'hawk-windows-x64.zip');
  fs.writeFileSync(zip, '这不是zip');
  const appDir = path.join(dir, 'app');
  fs.mkdirSync(appDir);
  const r = spawnSync(updaterExe, ['--pid', '0', '--zip', zip, '--app', appDir], { timeout: 60_000 });
  assert(r.status === 1, `坏包：非零退出（exit=${r.status}）`);
  const log = fs.readFileSync(path.join(dir, 'install.log'), 'utf8');
  assert(log.includes('安装失败'), `坏包：install.log 记录失败原因`);
}

fs.rmSync(work, { recursive: true, force: true });
if (failures > 0) {
  console.error(`\n${failures} 项断言失败`);
  process.exit(1);
}
console.log('\n全部通过');
