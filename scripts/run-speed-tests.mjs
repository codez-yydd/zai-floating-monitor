/**
 * 速度折叠常驻测试 runner（npm run test:speed 的第二步）：
 * 第一步 `tsc -p scripts/tsconfig.speed.json` 把断言脚本与被测模块
 * （CommonJS）编译到 .tmp-speed-test/（项目根，已 gitignore）。本脚本给
 * 输出目录写入 {"type":"commonjs"} 标记——项目根 package.json 为
 * type:module，不标记则 tsc 产出的 .js 会被按 ESM 解析而失败——随后加载
 * 断言脚本执行（断言失败抛错 → 进程非零退出 → npm script 失败）。
 * 全程只用 tsc 与 node，无任何第三方运行时依赖。
 */
import { mkdirSync, writeFileSync } from "node:fs";
import { fileURLToPath, pathToFileURL } from "node:url";
import { dirname, join } from "node:path";

const root = dirname(dirname(fileURLToPath(import.meta.url)));
const outDir = join(root, ".tmp-speed-test");
mkdirSync(outDir, { recursive: true });
writeFileSync(join(outDir, "package.json"), JSON.stringify({ type: "commonjs" }));

const entry = join(outDir, "scripts", "verify-speed-folds.js");
await import(pathToFileURL(entry).href);
console.log("test:speed 全部断言通过（修复 1 远端权重反例 + 修复 4 TTFT 加权反例 + 既有回归 + 幂等）");
