/**
 * 中文词典：按域 spread 合并，扁平点路径键（不嵌套），作为 en 的类型基准。
 */
import { common } from "./dicts/common";
import { layout } from "./dicts/layout";
import { stats } from "./dicts/stats";
import { summary } from "./dicts/summary";
import { pricing } from "./dicts/pricing";
import { sync } from "./dicts/sync";
import { compare } from "./dicts/compare";
import { report } from "./dicts/report";
import { settings } from "./dicts/settings";
import { theme } from "./dicts/theme";
import { projects } from "./dicts/projects";
import { share } from "./dicts/share";
import { credentials } from "./dicts/credentials";

export const zh = {
  ...common,
  ...layout,
  ...stats,
  ...summary,
  ...pricing,
  ...sync,
  ...compare,
  ...report,
  ...settings,
  ...theme,
  ...projects,
  ...share,
  ...credentials,
};
