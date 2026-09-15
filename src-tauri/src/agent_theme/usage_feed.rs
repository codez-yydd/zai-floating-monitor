//! 对话页用量统计条的数据源：皮肤已安装时后台轮询 ZCode 主库的
//! `turn_usage` 表（官方每轮聚合表，轮次完成时才落库），把最近 7 天内
//! （至多 3000 轮，超出保留最新）的轮次用量序列化为 `usage-data.js`
//! 写出到主题目录，供注入的 usage.js 周期加载并在对话区每轮下方渲染
//! 统计条。请求级速度另写入同目录的 `usage-speed.js` 小文件，速度更新
//! 不会牵连历史用量大文件。
//!
//! 数据契约（usage-data.js 内容，键名与 inject::USAGE_JS 消费端一字不差）：
//! ```js
//! window.__ZBAR_USAGE__ = { v: 2, ts: <最后数据变化时刻ms>, la: <最后活动时刻ms>,
//!   pu: <待处理用户消息时刻ms>|null（V5 附加字段，见下方 pu 信号说明）,
//!   ta: <活跃工具时刻ms>|null（V6 附加字段，见下方 ta/fe 信号说明）,
//!   fe: <失败轮事件时刻ms>|null（V6 附加字段，同上）,
//!   turns: [{
//!   umid: "msg_xxx",   用户消息 id（turn_usage.user_message_id，实测与
//!                      ZCode DOM 的 data-turn-id 同值，即渲染端匹配键；
//!                      列缺失或值为 null 时输出 null，该轮无法被匹配）
//!   turn: "turn_xxx",  轮 id（保留透出，不用于 DOM 匹配）
//!   sess: "sess_xxx",  status: "completed",
//!   start / end: 起止毫秒,
//!   in / out / cr / cw / rt: 输入(含缓存读) / 输出 / 缓存读 / 缓存写 / 推理，
//!     —— 均已并入该轮覆盖到的子代理聚合，
//!   req / retry / tool: 模型请求数 / 重试数 / 工具调用数（同样已并入），
//!   dur / ttft: 主轮自身总耗时 / 首字延迟毫秒（可能 null），
//!   sub: { n, req, in, out, cr, cw, rt } 并入的子代理聚合（可能 null），
//!   subagent: 1          仅子代理"自身视图行"携带（V20，其余行无此键）：
//!                         sess 为子代理会话 id、umid/数值/dur/ttft 均为
//!                         该子轮自身口径，与其并入主轮的行（sess=主会话、
//!                         数值含并入）并存导出。前端匹配不读取本键：
//!                         子代理详情面板靠自身行 umid 进入完成索引
//!                         （index）走完成态显示真实值，主会话视图按
//!                         sess 精确匹配不会命中自身行，无双计（见下方
//!                         子代理并入策略）；本键仅为数据侧自描述字段
//!                         （调试/未来消费预留），旧渲染脚本忽略未知
//!                         字段，v 保持 2
//!   models: "GLM-5.3,..."  该轮用到的模型（去重逗号拼接，含子代理），
//! }],
//!   runs: [{              进行中轮实时聚合（v2 格式不变的附加字段，旧
//!                         渲染脚本忽略未知字段，平滑兼容；空数组也输出）
//!   umid: "msg_xxx"|null  用户消息 id（model_usage.parent_user_message_id，
//!                         实测与 DOM data-turn-id 同值；子代理轮指向子代理
//!                         会话自己的消息，主会话 DOM 匹配不到，仅保留数据）
//!   sess: "sess_xxx",     会话 id（子代理进行中轮为 sess_subagent_* 形态）
//!   psess: "sess_xxx"|null 父会话 id（仅子代理会话查 session.parent_id，
//!                         主会话为 null；渲染端据此并入父会话累计）
//!   m: 1                  仅子代理行且父会话存在进行中主轮行时输出：
//!                         数值已并入该主轮行 sub，渲染端会话累计跳过
//!                         本行防双计（其余行无此键）
//!   sub: {n,req,in,out,cr,cw,rt}
//!                         仅主会话行输出：并入本主轮的子代理实时聚合
//!                         （子代理 runs 行按 psess 归并 + 游离子代理
//!                         完成轮，见下方 runs 侧并入策略；无并入则
//!                         无此键）
//!   in / out / cr / cw / rt: 该轮已完成模型请求的 token 合计（每完成一步
//!                         请求 model_usage 即落一行，2 秒轮询内可见）
//!   req: 行数,            已完成的模型请求数
//!   start: 首个请求开始毫秒
//! }],
//!   sess: [{              会话级统计（v2 格式不变的附加字段，旧渲染脚本
//!                         忽略未知字段平滑兼容；空数组也输出）。
//!   s: "sess_xxx",        会话 id（主会话行为"会话树"口径：自身 + 全部
//!                         子代理归并；子代理会话若出现在 turns/runs 亦
//!                         导出"自身树"行供其详情面板查询）
//!   —— model_usage 全量合计（含失败/中断轮。与 turns 的 turn_usage
//!      按轮聚合并存两套口径：turn_usage 覆盖不全（失败轮不落库），
//!      会话累计优先消费本通道，渲染端无本字段时回退旧口径）：
//!   tt: Σ = up+down+cr,   全量合计总量
//!   up / down / cr:       ↑ 非缓存输入（逐笔 clamp）/ ↓ 输出 / ⟲ 缓存读
//!   rq: 请求笔数          model_usage 行数（每行一笔请求）
//!   speed / speedState: V24.1 起大文件承载**稳定**速度——turns 行携带该
//!          完成轮最新请求的速度快照（轮完成后不再变化），sess 行携带
//!          空闲会话的速度/状态；runs 行不携带速度（秒级跳动值由
//!          usage-speed.js 旁路 1 秒覆盖）。旧文件（无这些键）由注入版按
//!          缺省 "–" 处理，首个导出周期（≤2 秒）自然补齐。
//! }] }
//! ```
//!
//! `usage-speed.js` 的内容是只含定位键、速度快照和状态的窄数据：
//! `window.__ZBAR_USAGE_SPEED__ = {v: 1, ts, turns, runs, sess}`。它不包含
//! 任何 token 合计，因此请求完成后只改写这个小文件；`quality` 为
//! `generation` 或 `request_average`，后者只表示包含请求等待时间的完成后
//! 平均值，界面带 `≈`，不宣称逐 Token 实时测速。
//! V24.1 起旁路进一步收窄（文件仍为 v1 形态，旧注入脚本可继续消费）：
//! - 历史轮（turns）的**稳定**速度快照随 2 秒大文件发布（完成轮的最新
//!   请求速度不再变化，天然稳定，不放大写盘频率）；
//! - 会话行的稳定速度/状态同样随大文件发布（空闲会话打开旧会话时仍能
//!   显示其最近一次确认速度）；`runs` 行的速度仍只经旁路 1 秒更新；
//! - 旁路只携带**活跃会话**（有进行中轮或刚发出待处理用户消息的会话）
//!   的 sess 条目与全部 runs 条目，`turns` 恒为空数组——大文件与旁路的
//!   优先级：旁路条目覆盖大文件（applySpeedData 仅按出现的键合并），旁
//!   路未覆盖的会话沿用大文件数值；新一轮开始时旁路 sess 条目携带显式
//!   null + measuring 清掉旧值，新轮没有已确认速度时不回填上一轮。
//!
//! ## la（最后活动时刻）字段（V2 附加字段，向后兼容）
//!
//! `la` = 数据快照内可见的最后真实活动时刻（全部完成轮 end 与全部
//! 进行中轮 start 的最大值）。宠物闲置判定（idle↔sleeping）消费它：
//! ts 是"数据内容变化时刻"，轮次滑出查询窗口边界（旧轮退出 turns
//! 序列）同样会刷新 ts——若无 la，每滑出一轮宠物都会从沉睡误弹回
//! 闲置并持续 IDLE_SLEEP_MS。la 只随真实活动前进，滑出窗口的旧轮
//! 不影响它（取 max 时旧值天然被压掉，轮滑出后 la 只会变小或不变，
//! 变小只会更快入睡，绝不误醒）。la 是 turns/runs 的纯推导值，天然
//! 随内容参与"变化才写盘"的对比（活动时刻变化本身就是数据变化）。
//! 进行中轮以该轮首请求时刻（runs.start）近似"最新请求时刻"：
//! runs 非空时宠物状态机短路进 working/typing，不消费 la，近似无
//! 影响；完成轮的 end 是精确值，闲置判定实际只消费它。旧消费端按
//! 未知字段忽略；旧数据文件（无 la）由宠物核心回退用 ts 判闲置。
//!
//! ## pu（待处理用户消息）信号（V5 附加字段，向后兼容）
//!
//! 宠物"进行中轮"（runs）通道只由**已完成的模型请求行**聚合（model_usage
//! 每笔请求完成才落行），而 `message` 表的 user 消息**发送即落库**（实测
//! time_created = 发送时刻）——用户发消息后首个模型请求要 30~70 秒才完成
//! 落库，期间 runs 为空，宠物停在 waiting/sleeping 造成"滞后"观感。pu =
//! 最近一条「尚无对应完成轮」的 user 消息的 time_created（毫秒），无则
//! null；宠物核心据此在 runs 为空时预判进入 working（PENDING_TURN_MS
//! 窗口内），消除首请求窗口的滞后。
//!
//! 查询方案（实测选定候选 A，候选 B 的 session_entry/input_history 表无
//! 更轻的用户输入信号——均无独立 role/时间语义可直接利用）：
//! - `message` 按 rowid DESC 取尾部 PENDING_SCAN_ROWS（64）行，子查询内
//!   `json_extract(data, '$.role')` 解析角色，外层过滤 role = 'user'；
//!   最近消息必在 rowid 尾部（发送即落库、rowid 单调递增），实测主库
//!   （10258 行 message，data 列均值 938 B）尾部 64 行含约 8 条 user
//!   消息，扫查成本与表大小无关（rowid 倒序索引扫），实测 100 次平均
//!   **0.023ms**，2 秒轮询周期 10ms 预算内余量充足，绝不全表扫；
//! - 完成轮匹配不回查库（turn_usage.user_message_id 无索引，回查即全表
//!   扫），而是与调用方已聚合的 turns 的 umid 集合（turn_usage.
//!   user_message_id，含子代理自身视图行）内存比对：待处理消息必是
//!   新近消息，其完成轮落库时同样新近，恒在查询窗口内；
//! - role = 'user' 实测全部为真实用户提示词（主会话与子代理会话的 user
//!   消息均带 agent 字段，工具结果不落 message 表），无伪信号；
//! - pu 参与内容对比（变化才写盘的 payload 含 pu——用户发消息本身就是
//!   数据变化，ts 随之刷新）与 la 推导（取 max，见上方 la 字段）；
//! - 降级：message 表/核心列缺失（老版本库）、查询失败（json1 异常等）
//!   一律返回 None（pu 信号缺失），不影响 turns/runs 导出；宠物核心对
//!   pu 缺失按旧数据文件兼容（预判不生效，行为同 V4）。
//!
//! ## ta / fe 信号（V6 附加字段，向后兼容）
//!
//! 宠物 V6 新增两个状态的数据通道（turns 行本就透出 status
//! :"completed"/"cancelled"/"error" 等原值，供渲染端与调试消费）：
//! - **ta（tool active，活跃工具）**：`tool_usage` 表的工具调用**开始瞬间
//!   落库**（status='running'、started_at 有值、completed_at 为 NULL），
//!   完成时更新为 completed——这是「正在执行工具（构建/测试/命令）」的
//!   实时信号，比 model_usage 的请求完成落库强得多（工具执行期间模型
//!   请求间隙，runs 的 out 停滞，宠物在 typing/working 间摇摆）。ta =
//!   最新一条 running 行的 started_at（毫秒），无则 null。查询走
//!   `tool_usage_started_tool_idx`（started_at 前缀索引，实测 10805 行
//!   主库平均 **0.003ms**），窗口 10 分钟兜底崩溃残留的 running 行
//!   （实库发现 3 天前的残留 running 行，正常完成时行会更新、窗口内
//!   自愈，崩溃残留由窗口剔除）。ta 与 pu 同款参与 ts/la 推导（工具
//!   开始也是活动）与内容对比（工具开始/结束都触发写盘刷新 ts）；
//! - **fe（failure event，失败轮事件）**：最近一次「失败或取消」完成轮
//!   的 completed_at（毫秒），判定 = turn_usage 行
//!   `status != 'completed' || cancelled_by_user = 1 || tool_error_count > 0`
//!   （status 为 cancelled/error，或成功轮内含取消/工具报错），无则
//!   null。**fe 只在失败轮新增时才变化**（MAX 语义天然满足：成功轮
//!   完成不命中条件、不刷新 fe），消费端以 now − fe < 3000ms 判断
//!   沮丧窗口；查询为 started_at 窗口过滤 + MAX 聚合（turn_usage 行
//!   数量级为每天几十行，实测 334 行主库平均 **0.018ms**），窗口取
//!   与 turns 相同的导出窗口（失败轮落库瞬间必在窗口内；超过窗口的
//!   超长失败轮漏判属可接受窄边缘）；
//! - 降级：tool_usage 表/核心列缺失（老版本库）、查询失败一律
//!   unwrap_or(None)（无 ta/fe → 宠物新状态不触发，行为同 V5），不阻塞
//!   turns/runs 导出；宠物核心对 ta/fe 缺失（旧数据文件）按 0 兼容。
//!
//! ## 心跳独立文件 usage-data-hb.js 与写盘策略（V2，向后兼容）
//!
//! 宠物陈旧判定（ZBar 退出 → 数据源停止 → 沉睡）需要每 2 秒刷新的
//! 心跳，但把心跳塞进 usage-data.js 会迫使大文件（典型几百 KB、至多
//! 3000 轮）每 2 秒全量重写（写放大，此前版本踩过的坑）。现拆为独立
//! 小文件：`window.__ZBAR_USAGE_HB__ = <ms>`（几十字节），注入版宠物
//! 壳每 2 秒经 script 时间戳重载读取喂给宠物核心（heartbeat 接口），
//! 核心缺失心跳时回退用 ts 判陈旧（V1 行为）。
//! - usage-data.js 恢复"内容不变跳写"原策略（ts 保持最后数据变化语义）；
//! - 心跳文件仅当注入版宠物开启（PetConfig.enabled && mode==injected，
//!   宠物配置统一收敛到 pet.json 后不再读 ThemeParams）时每周期重写；
//!   宠物关闭/悬浮窗形态时停止写并顺带清理残留文件（无常驻开销）；
//! - 写放大权衡：几十字节每 2 秒原子写可忽略；"降低心跳粒度"不可行
//!   （宠物阈值 10s 下粗粒度会在阈值边缘抖动误判）。
//! - 独立悬浮窗宠物（pet.rs 轮询器）经 Tauri 事件推流，事件本身自带
//!   新鲜度（查询成功才推），不消费心跳文件。
//!
//! ## runs（进行中轮）口径（实库验证结论）
//!
//! `model_usage` 每次模型请求完成即落一行（无 running 行），而
//! `turn_usage` 整轮结束才写：进行中的轮 = model_usage 有该 turn_id 的行
//! 且 turn_usage 尚无该 turn_id 的行。runs 取近 10 分钟窗口内按 turn_id
//! 分组的 model_usage 聚合，过滤掉已完成（done）轮；窗口放宽为"组内任一
//! 请求落在近 10 分钟 + 组行扫近 7 天"，覆盖长轮（>10 分钟）的完整聚合。
//! 轮完成后 turn_usage 行出现 → 该 turn_id 进入 done 集合 → runs 行消失、
//! turn_usage 行进入 turns（渲染端最终值无缝接管）。
//! 主会话行另有父会话保活分支（V20）：主轮派发子代理后自身静默等待，
//! 满 10 分钟会被上述窗口整体踢出 runs（前端每轮条跌回全 0 启动窗口
//! 占位、会话累计丢失该轮实时值，其子代理完成轮孤儿也随之失去挂载
//! 点）。故主会话行放宽为"该轮近 10 分钟有自身请求，或名下有子代理
//! 会话近 10 分钟在产生请求"（经 session.parent_id 查父会话集合）；
//! 子代理行与无子代理保活的僵尸主轮（异常中断、永不落库）仍维持
//! 10 分钟静默退出 runs。
//!
//! ## 子代理并入策略（实库验证结论，v3.10.1 主库实测）
//!
//! 子代理会话（session_id 形如 `sess_subagent_agent_<uuid>`）的轮次并入
//! 父会话中时间覆盖它的主轮（主轮 started_at ≤ 子轮 started_at 且
//! 子轮 completed_at ≤ 主轮 completed_at；父会话经 session.parent_id
//! 查得），同时另导出一条"自身视图行"（V20，sess/umid 为子代理自己的、
//! 数值为该子轮自身口径、带 subagent:1 标记（仅数据侧自描述，前端匹
//! 配不读该键，见数据契约）供子代理详情面板显示真实值——两行并存是
//! 预期：主轮行是"并入视图"，自身行是"自身视图"，会话累计按 sess
//! 精确匹配各只计一次。曾考虑的"精确关联"方案（子代理
//! model_usage.parent_user_message_id 指向父会话消息）
//! 经实库验证**不成立**：该字段指向的是子代理会话自己的消息（JOIN
//! message 后 msg_session = 子代理 session_id），无法回溯父会话，故并入
//! 采用时间窗口法（自身视图行不受影响，每条完成子轮都导出）。
//! 实测覆盖率 64/69（93%）：未命中的子轮均为合法边界——父会话轮尚未
//! 完成落库（随后续导出周期自然并入，全量重查 7 天窗口自带此自愈性）、
//! 父会话无 turn_usage 行（旧版本库）、子轮完成晚于父轮完成（后台代理
//! 越界运行）。未命中的子轮不并入主轮（孤儿分流给 runs 侧实时聚合，
//! 越界完成的维持丢弃防双计，见下节），自身视图行照常导出。
//!
//! ## runs 侧子代理并入（V9：主轮实时条与会话累计条实时反映子代理消耗）
//!
//! turns 的整轮并入只在主轮完成落库后发生；主轮进行期间需要把子代理
//! 消耗实时反映到主轮 runs 行上。口径：
//! - 每条主会话 runs 行新增 sub 聚合 = 所有 psess 指向该会话的子代理
//!   runs 行（一次会话并行多个子代理时全部并入）+ 游离子代理完成轮
//!   （子代理 turn_usage 行已落库、但所属主轮尚未落库而未被
//!   merge_subagent_turns 并入 turns 的部分；防双计判定沿用 merge 的
//!   时间窗口匹配逻辑：父会话存在"started_at ≤ 子轮 started_at <
//!   completed_at"的完成主轮 = 所属主轮已落库（子轮越界完成等边界），
//!   维持丢弃不输出；否则作为游离子代理完成轮输出给 runs 侧聚合）；
//! - 对应地，父会话存在进行中主轮行（同批 runs 内）的子代理 runs 行
//!   打 m:1 标记（其数值已并入主轮行 sub，渲染端会话累计跳过本行防
//!   双计）；父会话暂无主轮行（主轮首笔请求未完成）时不打标，渲染端
//!   按 psess 直接并入会话累计，主轮行出现后自动切换口径，无缝衔接；
//! - 子代理会话自己的 DOM（详情面板，同 document）渲染自身统计：进行
//!   中轮按子代理 runs 行 umid 匹配 live 条，完成轮按 turns 的自身视图
//!   行（V20，subagent:1）umid 命中完成索引走完成态，与主轮并入互不
//!   影响（同一数值两处展示是预期：主轮行是"并入视图"，子代理行是
//!   "自身视图"；会话累计按 sess 精确匹配只算一次）。
//!
//! ## 降级与健壮性
//!
//! - 老版本 ZCode 库可能没有 turn_usage 表或部分列：沿用 db::has_column
//!   的探测降级模式，表/核心列缺失时整个功能静默关闭（不导出不显示），
//!   非核心列缺失按 0 / NULL 降级；
//! - 主库只读连接（复用 zcode_sessions::open_main_db_readonly_uri），
//!   查询失败静默跳过本轮（下个周期重试），不 panic 不刷日志；
//! - 增量策略：每 2 秒全量重查最近 7 天窗口（至多 3000 轮、超出保留
//!   最新，防数据文件无限膨胀；长窗口覆盖打开旧会话的历史回填，优先
//!   简单方案，且天然覆盖"主轮晚于子轮落库"的并期场景）；序列化字节
//!   无变化时跳写（ts 保持最后数据变化语义，见上方心跳契约），写盘走
//!   .tmp + rename 原子替换。

use crate::agent_theme::store;
use rusqlite::Connection;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use crate::token_speed::{
    is_completed_status, request_speed_at, PENDING_FRESH_MS, RequestTiming, SpeedSnapshot,
    SpeedState,
};

/// 当前唯一支持的目标应用（与 mod.rs 注册表一致；feed 挂载点均由
/// agent_theme 的安装/卸载流程驱动，实际 app_id 恒为 zcode）
const TARGET_APP_ID: &str = "zcode";

/// 导出窗口：只导出最近 7 天的轮次（打开旧会话时历史轮普遍超过原 6 小时
/// 窗口导致统计条无数据，长窗口覆盖历史回填场景；更早的历史由 ZBar 面板
/// 统计覆盖）
const WINDOW_MS: i64 = 7 * 24 * 3600 * 1000;

/// 导出行数上限：窗口内轮次超过 3000 时仅保留最新 3000 轮（配合长窗口
/// 防数据文件无限膨胀；截断按 started_at 升序取末尾即最新的轮次）
const MAX_TURNS: usize = 3000;

/// runs（进行中轮）新鲜度窗口：该轮最新请求落在本窗口内才视为进行中
/// （防陈旧——异常中断的轮 model_usage 行不再增长，10 分钟后自动退出
/// runs 通道；正常轮远快于此，轮完成即由 done 集合过滤）
const RUN_WINDOW_MS: i64 = 10 * 60 * 1000;

/// pu（待处理用户消息）尾部扫查行数：最近消息必在 rowid 尾部（发送即
/// 落库、rowid 单调递增），64 行实测覆盖任意活跃期（真实主库尾部 64 行
/// 含约 8 条 user 消息）；rowid DESC LIMIT 为索引序扫，成本与表大小无关
/// （实测 10258 行主库平均 0.023ms，2 秒周期 10ms 预算内），禁止放宽为
/// 全表扫。见模块头 pu 信号说明
const PENDING_SCAN_ROWS: i64 = 64;

/// ta（活跃工具）查询窗口：running 行仅在 ZCode 崩溃/强杀时残留（实测
/// 主库发现 3 天前的残留行），正常完成即更新为 completed、ta 归 null；
/// 窗口剔除崩溃残留（值与 RUN_WINDOW_MS 同为 10 分钟"防陈旧残留"口径，
/// 独立定义防语义耦合）。消费端另有 TOOL_ACTIVE_MS（30s）二层兜底
pub(crate) const TOOL_WINDOW_MS: i64 = 10 * 60 * 1000;

/// 历史用量大文件刷新周期（毫秒）。速度小文件有独立更短节拍。
const INTERVAL_MS: u64 = 2000;

/// 速度小文件刷新周期（毫秒）：只读取活动会话的请求级时间字段，绝不
/// 重写 usage-data.js。可见页面的速度轮询也按此数量级运行。
const SPEED_INTERVAL_MS: u64 = 1000;

/// 导出连续失败的记日志间隔：连续失败达到该次数的整数倍时记一条 stderr
/// 日志（150 × INTERVAL_MS ≈ 5 分钟一条——瞬态失败保持静默不刷屏，
/// 持续死亡不再无声不可观测）；成功一轮即清零计数
const FEED_FAIL_LOG_EVERY: u64 = 150;

// ============================================================
// 导出数据结构（JSON 键名即 usage-data.js 契约，勿改）
// ============================================================

/// 导出的单轮用量。字段语义见模块头的数据契约。
#[derive(Debug, Clone, Serialize)]
pub(crate) struct UsageTurn {
    /// 轮 id（turn_usage.turn_id，保留透出；DOM data-turn-id 实测并非
    /// 此值，渲染端匹配不使用本字段）
    #[serde(rename = "turn")]
    pub(crate) turn_id: String,
    /// 用户消息 id（turn_usage.user_message_id，实测与 ZCode DOM 的
    /// data-turn-id 同值同源，渲染端匹配键；列缺失或值为 null 时导出
    /// null——该轮无法被 DOM 匹配，仅保留数据）
    #[serde(rename = "umid")]
    pub(crate) user_message_id: Option<String>,
    /// 会话 id
    #[serde(rename = "sess")]
    session_id: String,
    /// completed / cancelled / error 等（原样透出）
    status: String,
    /// 开始时刻（毫秒）
    start: i64,
    /// 完成时刻（毫秒，缺失为 null）
    end: Option<i64>,
    /// 输入 token（turn_usage 原始值，含缓存读；前端展示 ↑ = in − cr）
    #[serde(rename = "in")]
    input_tokens: i64,
    /// 输出 token
    #[serde(rename = "out")]
    pub(crate) output_tokens: i64,
    /// 缓存读 token
    #[serde(rename = "cr")]
    cache_read: i64,
    /// 缓存写 token
    #[serde(rename = "cw")]
    cache_write: i64,
    /// 推理 token
    #[serde(rename = "rt")]
    reasoning: i64,
    /// 模型请求数
    #[serde(rename = "req")]
    requests: i64,
    /// 模型重试数
    #[serde(rename = "retry")]
    retries: i64,
    /// 工具调用数
    #[serde(rename = "tool")]
    tool_calls: i64,
    /// 总耗时毫秒（主轮自身，不随并入变化；缺失为 null）
    dur: Option<i64>,
    /// 首字延迟毫秒（主轮自身，不随并入变化；缺失为 null）
    ttft: Option<i64>,
    /// 并入的子代理聚合（无并入为 null）
    pub(crate) sub: Option<SubAgg>,
    /// 子代理自身视图行标记（V20）：1 = 本行是子代理轮的"自身视图行"
    /// （sess 为子代理会话 id、umid/数值/dur/ttft 均为该子轮自身口径），
    /// 与其并入主轮的行（sess=主会话、umid=主轮消息id、数值含并入）并存
    /// 导出；前端匹配不读取本键：子代理详情面板靠自身行 umid 进入完成
    /// 索引（index）显示真实值，本键仅为数据侧自描述字段（调试/未来
    /// 消费预留）；主会话行无此键（None 不序列化，旧渲染脚本忽略未知
    /// 字段，v 保持 2 向后兼容）
    #[serde(rename = "subagent", skip_serializing_if = "Option::is_none")]
    pub(crate) subagent: Option<u8>,
    /// 该轮用到的模型（去重逗号拼接，含并入子代理的模型）
    models: String,
    /// 该轮最新模型请求的速度；没有有效请求级时间数据时省略。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) speed: Option<SpeedSnapshot>,
}

/// 子代理并入聚合明细（usage.js hover 展示用）
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct SubAgg {
    /// 并入的子代理轮数
    n: i64,
    /// 子代理模型请求数合计
    #[serde(rename = "req")]
    requests: i64,
    /// 子代理输入 token 合计
    #[serde(rename = "in")]
    input_tokens: i64,
    /// 子代理输出 token 合计
    #[serde(rename = "out")]
    pub(crate) output_tokens: i64,
    /// 子代理缓存读 token 合计
    #[serde(rename = "cr")]
    cache_read: i64,
    /// 子代理缓存写 token 合计
    #[serde(rename = "cw")]
    cache_write: i64,
    /// 子代理推理 token 合计
    #[serde(rename = "rt")]
    reasoning: i64,
}

impl SubAgg {
    fn empty() -> Self {
        SubAgg {
            n: 0,
            requests: 0,
            input_tokens: 0,
            output_tokens: 0,
            cache_read: 0,
            cache_write: 0,
            reasoning: 0,
        }
    }

    /// 累入一条子代理轮（n 计 1 轮，其余逐项累加；turns 整轮并入与
    /// runs 实时并入共用同一口径）
    fn add(
        &mut self,
        requests: i64,
        input_tokens: i64,
        output_tokens: i64,
        cache_read: i64,
        cache_write: i64,
        reasoning: i64,
    ) {
        self.n += 1;
        self.requests += requests;
        self.input_tokens += input_tokens;
        self.output_tokens += output_tokens;
        self.cache_read += cache_read;
        self.cache_write += cache_write;
        self.reasoning += reasoning;
    }
}

/// 从库中读出的子代理轮原始行（并入聚合前的中间形态）。V20 起额外
/// 携带自身视图行所需字段（session_id / status / user_message_id /
/// dur / ttft），见 collect_turns 的自身视图行生成。
#[derive(Debug, Clone)]
pub(crate) struct SubTurnRow {
    turn_id: String,
    /// 子代理会话 id（自身视图行的 sess 来源）
    session_id: String,
    /// 父会话 id（session.parent_id）
    parent_session_id: Option<String>,
    /// 轮状态（completed / cancelled 等，原样透出到自身视图行）
    status: String,
    started_at: i64,
    completed_at: Option<i64>,
    /// 子轮自己的用户消息 id（自身视图行的 umid；列缺失或值为 null 时
    /// 为 None，该轮无法与 DOM 匹配，仅保留数据）
    user_message_id: Option<String>,
    /// 子轮自身总耗时毫秒（自身视图行口径，列缺失为 null）
    dur: Option<i64>,
    /// 子轮自身首字延迟毫秒（自身视图行口径，列缺失为 null）
    ttft: Option<i64>,
    input_tokens: i64,
    output_tokens: i64,
    cache_read: i64,
    cache_write: i64,
    reasoning: i64,
    requests: i64,
    retries: i64,
    tool_calls: i64,
}

/// 进行中轮的实时聚合（runs 数组元素）。字段语义见模块头的数据契约；
/// JSON 键名即 usage.js 消费端契约，勿改。与 UsageTurn 的差异：无
/// status/dur/ttft/models（进行中轮无整轮聚合可读），多 psess（子代理
/// 进行中轮并入父会话累计的关联键）与 m（已并入主轮行 sub 的防双计
/// 标记）；sub 为 runs 侧实时并入的子代理聚合（结构与 UsageTurn.sub
/// 一致，仅主会话行携带）。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct UsageRun {
    /// Internal key used to look up the latest model request speed. It is not
    /// part of the usage-data.js contract.
    #[serde(skip)]
    pub(crate) turn_id: String,
    /// 用户消息 id（model_usage.parent_user_message_id，实测与 DOM
    /// data-turn-id 同值，渲染端匹配键；子代理轮指向子代理会话自己的
    /// 消息，主会话 DOM 匹配不到；列缺失或值为 null 时导出 null——
    /// 该轮无法与 DOM 匹配，仅保留数据并入会话累计）
    #[serde(rename = "umid")]
    pub(crate) user_message_id: Option<String>,
    /// 会话 id（子代理进行中轮为 sess_subagent_* 形态）
    #[serde(rename = "sess")]
    pub(crate) session_id: String,
    /// 父会话 id（仅子代理会话查 session.parent_id 得出，主会话为 null；
    /// 渲染端按 sess 或 psess 命中当前会话并入累计）
    #[serde(rename = "psess")]
    parent_session_id: Option<String>,
    /// 输入 token 合计（该轮已完成模型请求，含缓存读；展示 ↑ = in − cr）
    #[serde(rename = "in")]
    input_tokens: i64,
    /// 输出 token 合计
    #[serde(rename = "out")]
    pub(crate) output_tokens: i64,
    /// 缓存读 token 合计
    #[serde(rename = "cr")]
    cache_read: i64,
    /// 缓存写 token 合计
    #[serde(rename = "cw")]
    cache_write: i64,
    /// 推理 token 合计
    #[serde(rename = "rt")]
    reasoning: i64,
    /// 模型请求数（model_usage 行数）
    #[serde(rename = "req")]
    requests: i64,
    /// 本轮首个（扫查窗口内）请求开始时刻（毫秒）
    start: i64,
    /// 已并入父会话主轮行 sub 的标记（1 = 本行数值已并入对应主轮行
    /// sub，渲染端会话累计跳过本行防双计）。仅子代理行且父会话存在
    /// 进行中主轮行时置位；None 不序列化（主会话行与无主轮行的子代理
    /// 行均无此键）
    #[serde(rename = "m", skip_serializing_if = "Option::is_none")]
    pub(crate) merged: Option<u8>,
    /// 并入本主轮行的子代理实时聚合（仅主会话行：同批子代理 runs 行
    /// 按 psess 归并 + 游离子代理完成轮；None 不序列化）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) sub: Option<SubAgg>,
    /// 该进行中轮最新模型请求的速度；没有有效请求级时间数据时省略。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) speed: Option<SpeedSnapshot>,
}

// ============================================================
// 后台任务（普通 thread + flag 模式，沿用项目 spawn_sync_worker 惯例）
// ============================================================

/// 停止标记：卸载/还原皮肤时置位；线程完成当前导出周期（含 DB busy
/// 等待，最长约 3 秒余）后，在下一个检查点退出
static FEED_STOP: AtomicBool = AtomicBool::new(false);
/// 导出线程句柄（含已退出的旧句柄，start 时检测并复用槽位，防重复起线程）
static FEED_HANDLE: OnceLock<Mutex<Option<thread::JoinHandle<()>>>> = OnceLock::new();

fn feed_handle() -> &'static Mutex<Option<thread::JoinHandle<()>>> {
    FEED_HANDLE.get_or_init(|| Mutex::new(None))
}

/// 应用启动挂点：皮肤已安装时启动导出线程（未安装不启动，零开销）。
pub fn start_if_installed() {
    if store::load_state(TARGET_APP_ID).is_installed() {
        start();
    }
}

/// 启动导出线程（安装成功挂点调用）。已在运行时为幂等 no-op（只清掉可能
/// 残留的停止标记，覆盖 stop 后线程未退完又立即 start 的窄窗口）。
pub fn start() {
    let mut guard = match feed_handle().lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };
    if guard.as_ref().is_some_and(|h| !h.is_finished()) {
        FEED_STOP.store(false, Ordering::Relaxed);
        return;
    }
    FEED_STOP.store(false, Ordering::Relaxed);
    // 启动失败仅放弃本功能（不 panic 不阻塞调用方），后续挂点可重试
    if let Ok(h) = thread::Builder::new()
        .name("zbar-usage-feed".into())
        .spawn(feed_loop)
    {
        *guard = Some(h);
    }
}

/// 停止导出线程（卸载/还原成功挂点调用）。仅置位停止标记，不 join 不等待；
/// 线程完成当前导出周期（含可能的 DB busy 等待，最长约 3 秒余）后，
/// 于下一个检查点退出。
pub fn stop() {
    FEED_STOP.store(true, Ordering::Relaxed);
}

fn feed_loop() {
    // 变化检测缓存：线程生命周期内持有上轮 turns+runs 序列化字节（线程
    // 重启丢失缓存只多写一次盘，无正确性影响）
    let mut cache: Option<String> = None;
    // 速度小文件单独维护：每秒只查活动会话的请求级字段，
    // usage-data.js 仍按 2 秒历史导出节拍运行。
    let mut speed_view = SpeedView::default();
    let mut speed_cache: Option<String> = None;
    let mut last_full = Instant::now() - Duration::from_millis(INTERVAL_MS);
    let mut last_speed = Instant::now() - Duration::from_millis(SPEED_INTERVAL_MS);
    loop {
        if FEED_STOP.load(Ordering::Relaxed) {
            return;
        }
        // 每轮再核对安装状态：皮肤被异常还原（state 复位而未走 stop 挂点）
        // 时自动退出，不留空转线程
        if !store::load_state(TARGET_APP_ID).is_installed() {
            return;
        }
        let now = Instant::now();
        if now.duration_since(last_full).as_millis() >= INTERVAL_MS as u128 {
            export_once(&mut cache, &mut speed_view, &mut speed_cache);
            last_full = now;
            last_speed = now;
        } else if now.duration_since(last_speed).as_millis() >= SPEED_INTERVAL_MS as u128 {
            export_speed_once(&mut speed_view, &mut speed_cache);
            last_speed = now;
        }
        // 分段睡眠：sleep 期间可及时感知 stop（导出期间不响应）。
        // stop 仅置位 flag、不 join 不等待：线程在完成当前导出周期后于下个
        // 检查点退出（应用退出时进程随之结束，与项目其它后台线程同款）。
        for _ in 0..10 {
            if FEED_STOP.load(Ordering::Relaxed) {
                return;
            }
            thread::sleep(Duration::from_millis(100));
        }
    }
}

/// 导出连续失败计数（export_once 专用）：成功清零、失败自增，达到
/// FEED_FAIL_LOG_EVERY 整数倍时记一条日志。局部静态侵入最小，导出线程
/// 是唯一调用方，无并发竞争
static FEED_FAIL_COUNT: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Serialize)]
struct SpeedRunEntry {
    #[serde(skip)]
    turn_id: String,
    #[serde(rename = "umid")]
    user_message_id: Option<String>,
    #[serde(rename = "sess")]
    session_id: String,
    start: i64,
    speed: Option<SpeedSnapshot>,
}

#[derive(Debug, Clone, Serialize)]
struct SpeedSessionEntry {
    #[serde(rename = "s")]
    session_id: String,
    speed: Option<SpeedSnapshot>,
    #[serde(rename = "speedState")]
    speed_state: SpeedState,
    /// Internal only: a root session's speed includes every descendant.
    #[serde(skip)]
    member_session_ids: Vec<String>,
    /// Internal only: while a pending user message is the current round,
    /// older requests are not eligible for this session speed.
    #[serde(skip)]
    round_started_at: Option<i64>,
    /// Internal only: independent "a round is actually running" flag. It is
    /// never derived from `speed_state`（A2）：一轮正在生成且已有一次完成
    /// 请求时状态是 Recent，此时若最新有效速度暂时为空，状态必须回落到
    /// Measuring 而不是 Unavailable。
    #[serde(skip)]
    generating: bool,
}

/// The last full export's small speed shape. It is also the target set for
/// one-second speed refreshes, so a request that finishes between two full
/// history exports can update visible speed without rebuilding `turns/runs`.
/// V24.1 起只承载活跃通道（A3）：`turns` 不再进入旁路（历史轮的稳定速度
/// 随 2 秒大文件发布），`sessions` 只保留活跃会话（有进行中轮成员或刚发
/// 出待处理用户消息的会话）。
#[derive(Debug, Clone, Default)]
struct SpeedView {
    runs: Vec<SpeedRunEntry>,
    sessions: Vec<SpeedSessionEntry>,
}

impl SpeedView {
    /// 构建旁路视图。`pending_session` 是刚发出待处理用户消息的会话
    /// （首请求等待期需要在 1 秒拍上看到状态与首个速度）。
    fn from_rows(
        runs: &[UsageRun],
        sessions: &[UsageSessionStat],
        pending_session: Option<&str>,
    ) -> Self {
        let active_members: BTreeSet<&str> =
            runs.iter().map(|run| run.session_id.as_str()).collect();
        Self {
            runs: runs
                .iter()
                .map(|run| SpeedRunEntry {
                    turn_id: run.turn_id.clone(),
                    user_message_id: run.user_message_id.clone(),
                    session_id: run.session_id.clone(),
                    start: run.start,
                    speed: run.speed.clone(),
                })
                .collect(),
            sessions: sessions
                .iter()
                .filter(|session| {
                    pending_session == Some(session.session_id.as_str())
                        || session
                            .member_session_ids
                            .iter()
                            .any(|member| active_members.contains(&member.as_str()))
                })
                .map(|session| SpeedSessionEntry {
                    session_id: session.session_id.clone(),
                    speed: session.speed.clone(),
                    speed_state: session.speed_state,
                    member_session_ids: session.member_session_ids.clone(),
                    round_started_at: session.round_started_at,
                    generating: session.generating,
                })
                .collect(),
        }
    }

    /// 旁路 1 秒拍需要读取请求级字段的会话集合：活跃会话条目自身及其全部
    /// 树成员（会话级速度沿树取最新），外加进行中轮所在会话。绝不包含
    /// 历史轮的会话（大文件已覆盖）。
    fn session_ids(&self) -> BTreeSet<String> {
        let mut ids = BTreeSet::new();
        for session in &self.sessions {
            ids.insert(session.session_id.clone());
            ids.extend(session.member_session_ids.iter().cloned());
        }
        for run in &self.runs {
            ids.insert(run.session_id.clone());
        }
        ids.retain(|id| !id.is_empty());
        ids
    }

    fn refresh(&mut self, catalog: &SpeedCatalog, observed_at_ms: i64) {
        for run in &mut self.runs {
            run.speed = speed_for_latest(
                catalog.for_turn(&run.session_id, &run.turn_id),
                observed_at_ms,
            );
        }
        for session in &mut self.sessions {
            session.speed = speed_for_latest(
                if session.member_session_ids.is_empty() {
                    catalog.for_session(&session.session_id)
                } else {
                    catalog.latest_for_members(
                        &session.member_session_ids,
                        session.round_started_at,
                    )
                },
                observed_at_ms,
            );
            session.speed_state = if session.speed.is_some() {
                SpeedState::Recent
            } else if session.generating {
                SpeedState::Measuring
            } else {
                SpeedState::Unavailable
            };
        }
    }
}

/// 单轮导出：读库 → 序列化 → flush_export 落盘（大文件按变化写、心跳
/// 小文件按宠物开关维护）。任何失败静默跳过本轮（下个周期重试），不
/// panic；连续失败达 FEED_FAIL_LOG_EVERY 整数倍时记一条日志（防再次
/// 无声死亡——此前 NULL turn_id 脏行曾让导出静默停更且完全不可观测）。
fn export_once(
    cache: &mut Option<String>,
    speed_view: &mut SpeedView,
    speed_cache: &mut Option<String>,
) {
    let result = (|| -> Result<(), String> {
        let conn = crate::zcode_sessions::open_main_db_readonly_uri()?;
        let now_ms = chrono::Utc::now().timestamp_millis();
        // turns + 游离子代理完成轮（turns/merge 阶段分流输出，见
        // merge_subagent_turns）；None = turn_usage 表/核心列缺失（老版本
        // ZCode），功能静默关闭
        let Some((mut turns, sub_orphans)) = collect_turns_raw(&conn, now_ms - WINDOW_MS)? else {
            return Ok(());
        };
        // 进行中轮 runs：与 turns 同连接同轮询周期读出。刻意不做"失败降级
        // 空数组"——runs 与 turns 任一查询失败都整体跳过本轮（下周期重试），
        // 避免 runs 闪空导致渲染端实时段闪烁断档
        let done = collect_done_turn_ids(&conn, now_ms - WINDOW_MS)?;
        let mut runs = collect_runs_raw(
            &conn,
            now_ms - RUN_WINDOW_MS,
            now_ms - WINDOW_MS,
            &done,
            &sub_orphans,
        )?;
        let dir = store::app_dir(TARGET_APP_ID)?;
        fs::create_dir_all(&dir).map_err(|e| format!("创建主题目录失败: {e}"))?;
        // 心跳写出条件：注入版形态开启（宠物配置统一收敛到 pet.json，
        // ThemeParams 不再承载宠物参数；悬浮窗形态经 Tauri 事件推流不
        // 消费心跳文件）
        let pet_enabled = crate::pet::load_pet_config().wants_injected_pet();
        // pu（待处理用户消息）：与 turns 同轮询周期读出；完成轮匹配复用
        // 已聚合的 turns umid 集合（内存比对，不回查库）。查询失败按无
        // 信号降级（unwrap_or(None)）——pu 是附加信号，失败不阻塞
        // turns/runs 导出，宠物端按 pu 缺失退化为既有行为。
        // A1 新鲜期：待处理消息只在 PENDING_FRESH_MS 内独立成立；超期后
        // 除非其会话树内仍有进行中轮（runs，经会话树根归并的"可复核活跃
        // 证据"），否则不再冒充活跃轮——pu 透出 null、轮状态回落空闲，
        // 会话全生命周期累计不受影响（sess 聚合独立于 pu 存在）
        let done_umids: BTreeSet<String> = turns
            .iter()
            .filter_map(|t| t.user_message_id.clone())
            .collect();
        let tree_index = crate::token_speed::load_session_tree_index(&conn)?;
        let mut corroborating_sessions: BTreeSet<String> = BTreeSet::new();
        for run in &runs {
            corroborating_sessions.insert(run.session_id.clone());
            if tree_index.has_parent() {
                corroborating_sessions.insert(tree_index.root_for(&run.session_id));
            }
        }
        let pending_user = collect_pending_user(&conn, &done_umids, now_ms, &corroborating_sessions)
            .unwrap_or(None);
        // ta/fe（V6 附加信号）：与 pu 同款降级（查询失败按无信号，不阻塞
        // turns/runs 导出）；ta 窗口为 10 分钟残留兜底、fe 窗口与 turns
        // 相同（失败轮落库瞬间必在窗口内）
        let active_tool = collect_active_tool_ms(&conn, now_ms - TOOL_WINDOW_MS).unwrap_or(None);
        let failure_event = collect_failure_event_ms(&conn, now_ms - WINDOW_MS).unwrap_or(None);
        // 会话级统计（sess：model_usage 全量合计）：附加通道，查询
        // 失败降级为空数组（不阻塞 turns/runs 导出——渲染端对无 sess 数据
        // 回退旧 sessionTotals 口径），瞬时闪空仅回退口径一轮
        let pending_session = pending_user.as_ref().map(|pending| pending.session_id.as_str());
        // One session-qualified SpeedCatalog supplies turns, runs and sess in
        // this export. In particular, no bare `turn_id IN (...)` query is
        // issued against the `(session_id, turn_id)` index.
        let speed_sessions = session_ids_for_stats(&conn, &turns, &runs, pending_session)?;
        let speed_catalog = SpeedCatalog::load(&conn, &speed_sessions).unwrap_or_default();
        attach_turn_speeds(&speed_catalog, &mut turns, now_ms);
        attach_run_speeds(&speed_catalog, &mut runs, now_ms);
        let sess = collect_session_stats_with_catalog(
            &conn,
            &turns,
            &runs,
            &speed_catalog,
            pending_user.as_ref(),
        )
        .unwrap_or_default();
        let next_speed_view = SpeedView::from_rows(&runs, &sess, pending_session);
        flush_export(
            &dir,
            cache,
            pet_enabled,
            &turns,
            &runs,
            &sess,
            pending_user.as_ref().map(|pending| pending.time_created),
            active_tool,
            failure_event,
            now_ms,
        )?;
        *speed_view = next_speed_view;
        let _ = write_speed_view(speed_cache, &dir, speed_view, now_ms);
        Ok(())
    })();
    match result {
        Ok(()) => FEED_FAIL_COUNT.store(0, Ordering::Relaxed),
        Err(e) => {
            // 静默跳过本轮（库被锁超时、目录暂不可写等瞬态），下个周期
            // 重试；仅连续失败达到 FEED_FAIL_LOG_EVERY 整数倍（约 5 分钟
            // 一条）时记日志，瞬态失败不刷屏
            let n = FEED_FAIL_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
            if n % FEED_FAIL_LOG_EVERY == 0 {
                eprintln!(
                    "[zbar-usage-feed] 导出已连续失败 {n} 轮（每 {INTERVAL_MS}ms 重试）: {e}"
                );
            }
        }
    }
}

/// One-second side channel. It only reads request timing/output columns for
/// the last full export's active session set and writes `usage-speed.js`; the
/// large seven-day history file is not touched.
fn export_speed_once(speed_view: &mut SpeedView, speed_cache: &mut Option<String>) {
    if speed_view.session_ids().is_empty() {
        return;
    }
    let result = (|| -> Result<(), String> {
        let conn = crate::zcode_sessions::open_main_db_readonly_uri()?;
        let now_ms = chrono::Utc::now().timestamp_millis();
        let sessions = speed_view.session_ids();
        let catalog = SpeedCatalog::load(&conn, &sessions)?;
        speed_view.refresh(&catalog, now_ms);
        let dir = store::app_dir(TARGET_APP_ID)?;
        fs::create_dir_all(&dir).map_err(|e| format!("创建主题目录失败: {e}"))?;
        write_speed_view(speed_cache, &dir, speed_view, now_ms).map(|_| ())
    })();
    let _ = result;
}

/// 导出结果落盘（不碰数据库，供单元测试复用）：
/// - 大文件 usage-data.js 按变化写（内容不变跳写，ts 保持最后数据
///   变化语义；la/pu/ta/fe 随内容透出——pu/ta/fe 参与内容对比，用户
///   发消息、工具开始/结束、失败轮落库本身就是数据变化；sess 为会话
///   级统计附加数组，同样参与内容对比）；
/// - 心跳小文件 usage-data-hb.js 仅注入版宠物开启时每周期无条件重写
///   （大文件跳写周期里心跳仍独立推进）；宠物关闭时停写并清理残留
///   （remove 不存在的文件是常态失败，忽略）。
pub(crate) fn flush_export(
    dir: &Path,
    cache: &mut Option<String>,
    pet_enabled: bool,
    turns: &[UsageTurn],
    runs: &[UsageRun],
    sess: &[UsageSessionStat],
    pending_user: Option<i64>,
    active_tool: Option<i64>,
    failure_event: Option<i64>,
    now_ms: i64,
) -> Result<(), String> {
    let la = last_activity_ms(turns, runs, pending_user, active_tool);
    // A3 大小文件分工：完成轮（turns）的最新请求速度与空闲会话（sess）的
    // 速度/状态都是稳定值——它们随 2 秒大文件发布且只在真实数据变化（请
    // 求落库/轮结束/会话合计推进）时才参与写盘对比，不放大写盘频率；
    // 进行中轮（runs）的速度是秒级跳动值，仍只经 usage-speed.js 旁路发
    // 布，大文件中的 runs 行不携带速度（旁路按 umid 覆盖）。
    let mut large_runs = runs.to_vec();
    for run in &mut large_runs {
        run.speed = None;
    }
    let turns_json =
        serde_json::to_string(turns).map_err(|e| format!("序列化用量数据失败: {e}"))?;
    let runs_json = serde_json::to_string(&large_runs)
        .map_err(|e| format!("序列化进行中轮失败: {e}"))?;
    let sess_json =
        serde_json::to_string(sess).map_err(|e| format!("序列化会话统计失败: {e}"))?;
    write_if_changed(
        dir, cache, la, pending_user, active_tool, failure_event, &turns_json, &runs_json,
        &sess_json, now_ms,
    )?;
    if pet_enabled {
        write_heartbeat_file(dir, now_ms)?;
    } else {
        let _ = fs::remove_file(dir.join(store::USAGE_HB_FILE));
    }
    Ok(())
}

// ============================================================
// 读库与聚合（纯逻辑拆分便于单元测试，不依赖真实 ~/.zcode）
// ============================================================

/// 探测表是否存在（table 为代码内常量，无注入风险）
fn has_table(conn: &Connection, table: &str) -> bool {
    conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
        [table],
        |row| row.get::<_, i64>(0),
    )
    .map(|c| c > 0)
    .unwrap_or(false)
}

/// 数值列探测降级表达式：列存在取 COALESCE(col, 0)，缺失取常量 0
/// （老版本 turn_usage 可能缺部分统计列，按现有 has_column 模式降级）
fn num_col(conn: &Connection, table: &str, col: &str) -> String {
    if crate::db::has_column(conn, table, col) {
        format!("COALESCE({col}, 0)")
    } else {
        "0".to_string()
    }
}

/// 可空列探测降级表达式：列存在取列名，缺失取 NULL
fn opt_col(conn: &Connection, table: &str, col: &str) -> String {
    if crate::db::has_column(conn, table, col) {
        col.to_string()
    } else {
        "NULL".to_string()
    }
}

#[derive(Debug, Clone)]
struct LatestUsageRequest {
    order: i64,
    timing: RequestTiming,
}

/// All request-speed lookups needed by one usage export. The old implementation
/// ran one `turn_id IN (...)` query for completed turns, another for runs, and a
/// third `session_id IN (...)` query for session rows. `turn_id` is not the
/// leading column of ZCode's `(session_id, turn_id)` index, so the first two
/// queries could scan the whole table. This catalog performs one session-
/// qualified read per export and indexes the result by both keys in memory.
/// The query therefore uses the leading session column of the real index and
/// never treats a bare turn id as globally indexed.
#[derive(Debug, Clone, Default)]
struct SpeedCatalog {
    by_turn: BTreeMap<(String, String), LatestUsageRequest>,
    by_session: BTreeMap<String, LatestUsageRequest>,
}

impl SpeedCatalog {
    fn load(conn: &Connection, session_ids: &BTreeSet<String>) -> Result<Self, String> {
        if session_ids.is_empty()
            || !has_table(conn, "model_usage")
            || !crate::db::has_column(conn, "model_usage", "session_id")
            || !crate::db::has_column(conn, "model_usage", "started_at")
        {
            return Ok(Self::default());
        }
        let (out, first, completed, duration, status) = (
            num_col(conn, "model_usage", "output_tokens"),
            opt_col(conn, "model_usage", "first_token_at"),
            opt_col(conn, "model_usage", "completed_at"),
            opt_col(conn, "model_usage", "duration_ms"),
            opt_col(conn, "model_usage", "status"),
        );
        let turn = opt_col(conn, "model_usage", "turn_id");
        let placeholders = vec!["?"; session_ids.len()].join(", ");
        // Do not add ORDER BY: the composite session/turn index can deliver
        // the session-qualified rows directly; latest selection is stable in
        // Rust and does not need a temporary sort.
        let sql = format!(
            "SELECT session_id, {turn}, {out}, started_at, {first}, {completed}, {duration}, {status} \
             FROM model_usage WHERE session_id IN ({placeholders})"
        );
        let mut stmt = conn
            .prepare(&sql)
            .map_err(|error| format!("准备请求速度查询失败: {error}"))?;
        let rows = stmt
            .query_map(rusqlite::params_from_iter(session_ids.iter()), |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, Option<i64>>(3)?,
                    row.get::<_, Option<i64>>(4)?,
                    row.get::<_, Option<i64>>(5)?,
                    row.get::<_, Option<i64>>(6)?,
                    row.get::<_, Option<String>>(7)?,
                ))
            })
            .map_err(|error| format!("读取请求速度失败: {error}"))?;

        let mut catalog = Self::default();
        for row in rows {
            let (
                Some(session_id),
                turn_id,
                output_tokens,
                started_at,
                first_token_at,
                completed_at,
                duration_ms,
                status,
            ) = row.map_err(|error| format!("读取请求速度失败: {error}"))?
            else {
                continue;
            };
            if session_id.is_empty() {
                continue;
            }
            let completed_status = is_completed_status(status.as_deref());
            let timing = RequestTiming {
                // Keep the newest non-terminal row as an invalid candidate.
                // Otherwise an older completed request would remain visible
                // while a newer request in the same turn is still running.
                output_tokens: completed_status.then_some(output_tokens.max(0)).unwrap_or(0),
                started_at,
                first_token_at,
                completed_at,
                duration_ms,
                request_id: None,
            };
            let candidate = LatestUsageRequest {
                order: request_order(&timing),
                timing,
            };
            update_latest(&mut catalog.by_session, session_id.clone(), candidate.clone());
            if let Some(turn_id) = turn_id.filter(|value| !value.is_empty()) {
                update_latest(
                    &mut catalog.by_turn,
                    (session_id, turn_id),
                    candidate,
                );
            }
        }
        Ok(catalog)
    }

    fn for_turn(&self, session_id: &str, turn_id: &str) -> Option<&LatestUsageRequest> {
        self.by_turn
            .get(&(session_id.to_string(), turn_id.to_string()))
    }

    fn for_session(&self, session_id: &str) -> Option<&LatestUsageRequest> {
        self.by_session.get(session_id)
    }

    fn latest_for_members(
        &self,
        members: &[String],
        round_started_at: Option<i64>,
    ) -> Option<&LatestUsageRequest> {
        members
            .iter()
            .filter_map(|member| self.for_session(member))
            .filter(|request| {
                round_started_at.is_none_or(|started| {
                    request
                        .timing
                        .started_at
                        .is_some_and(|request_started| request_started >= started)
                })
            })
            .max_by_key(|request| request.order)
    }
}

fn request_order(timing: &RequestTiming) -> i64 {
    timing
        .completed_at
        .filter(|value| *value > 0)
        .or_else(|| {
            timing.started_at.and_then(|start| {
                timing
                    .duration_ms
                    .filter(|value| *value > 0)
                    .and_then(|duration| start.checked_add(duration))
            })
        })
        .or(timing.started_at.filter(|value| *value > 0))
        .unwrap_or(0)
}

fn update_latest<K: Ord>(
    map: &mut BTreeMap<K, LatestUsageRequest>,
    key: K,
    candidate: LatestUsageRequest,
) {
    if map
        .get(&key)
        .is_none_or(|previous| candidate.order >= previous.order)
    {
        map.insert(key, candidate);
    }
}

fn speed_for_latest(
    request: Option<&LatestUsageRequest>,
    observed_at_ms: i64,
) -> Option<SpeedSnapshot> {
    request.and_then(|latest| request_speed_at(&latest.timing, observed_at_ms))
}

fn session_ids_for_rows(
    turns: &[UsageTurn],
    runs: &[UsageRun],
    extra: Option<&str>,
) -> BTreeSet<String> {
    let mut ids = BTreeSet::new();
    ids.extend(turns.iter().map(|turn| turn.session_id.clone()));
    ids.extend(runs.iter().map(|run| run.session_id.clone()));
    if let Some(id) = extra.filter(|id| !id.is_empty()) {
        ids.insert(id.to_string());
    }
    ids
}

fn session_ids_for_stats(
    conn: &Connection,
    turns: &[UsageTurn],
    runs: &[UsageRun],
    pending_session: Option<&str>,
) -> Result<BTreeSet<String>, String> {
    let appeared = session_ids_for_rows(turns, runs, pending_session);
    if appeared.is_empty() {
        return Ok(appeared);
    }
    let tree_index = crate::token_speed::load_session_tree_index(conn)?;
    let mut targets = appeared.clone();
    if tree_index.has_parent() {
        for id in &appeared {
            targets.insert(tree_index.root_for(id));
        }
    }
    let mut members = BTreeSet::new();
    for target in targets {
        members.extend(tree_index.members(&target));
    }
    Ok(members)
}

fn attach_turn_speeds(catalog: &SpeedCatalog, turns: &mut [UsageTurn], observed_at_ms: i64) {
    for turn in turns {
        turn.speed = speed_for_latest(
            catalog.for_turn(&turn.session_id, &turn.turn_id),
            observed_at_ms,
        );
    }
}

fn attach_run_speeds(catalog: &SpeedCatalog, runs: &mut [UsageRun], observed_at_ms: i64) {
    for run in runs {
        run.speed = speed_for_latest(
            catalog.for_turn(&run.session_id, &run.turn_id),
            observed_at_ms,
        );
    }
}

/// 读出最近窗口内的主会话轮 + 并入子代理轮 + 模型清单，返回
/// (导出序列, 游离子代理完成轮)。游离子代理完成轮 = 子代理 turn_usage
/// 行已落库、但所属主轮尚未落库未被并入 turns 的部分（merge 阶段分流，
/// 见 merge_subagent_turns），交由 runs 侧聚合进主轮行 sub。
/// Ok(None) = 功能关闭（turn_usage 表或核心列缺失）；
/// Err = 瞬态查询失败（调用方静默跳过本轮）。
/// Public compatibility wrapper used by the pet worker and older call sites.
/// The pet contract does not contain speed fields, so this path deliberately
/// returns the count-only rows. The main usage export calls `collect_turns_raw`
/// and shares one SpeedCatalog with runs and session statistics instead of
/// repeating the speed query.
pub(crate) fn collect_turns(
    conn: &Connection,
    window_start_ms: i64,
) -> Result<Option<(Vec<UsageTurn>, Vec<SubTurnRow>)>, String> {
    collect_turns_raw(conn, window_start_ms)
}

#[cfg(test)]
fn collect_turns_with_speed(
    conn: &Connection,
    window_start_ms: i64,
) -> Result<Option<(Vec<UsageTurn>, Vec<SubTurnRow>)>, String> {
    let Some((mut turns, sub_orphans)) = collect_turns_raw(conn, window_start_ms)? else {
        return Ok(None);
    };
    let session_ids = session_ids_for_rows(&turns, &[], None);
    let catalog = SpeedCatalog::load(conn, &session_ids)?;
    attach_turn_speeds(&catalog, &mut turns, chrono::Utc::now().timestamp_millis());
    Ok(Some((turns, sub_orphans)))
}

fn collect_turns_raw(
    conn: &Connection,
    window_start_ms: i64,
) -> Result<Option<(Vec<UsageTurn>, Vec<SubTurnRow>)>, String> {
    // 功能总开关：表或任一核心列缺失 → 整个功能静默关闭（不导出不显示）
    if !has_table(conn, "turn_usage")
        || !crate::db::has_column(conn, "turn_usage", "session_id")
        || !crate::db::has_column(conn, "turn_usage", "turn_id")
        || !crate::db::has_column(conn, "turn_usage", "status")
        || !crate::db::has_column(conn, "turn_usage", "started_at")
    {
        return Ok(None);
    }

    // ---- 主会话轮（子代理会话整段排除，sub 前缀覆盖
    //      sess_subagent_agent_ 及将来可能的其它子代理形态）----
    let (inp, out, rt) = (
        num_col(conn, "turn_usage", "input_tokens"),
        num_col(conn, "turn_usage", "output_tokens"),
        num_col(conn, "turn_usage", "reasoning_tokens"),
    );
    let (cw, cr) = (
        num_col(conn, "turn_usage", "cache_creation_input_tokens"),
        num_col(conn, "turn_usage", "cache_read_input_tokens"),
    );
    let (req, retry, tool) = (
        num_col(conn, "turn_usage", "model_request_count"),
        num_col(conn, "turn_usage", "model_retry_count"),
        num_col(conn, "turn_usage", "tool_call_count"),
    );
    let completed = opt_col(conn, "turn_usage", "completed_at");
    let dur = opt_col(conn, "turn_usage", "duration_ms");
    let ttft = opt_col(conn, "turn_usage", "time_to_first_token_ms");
    // umid（用户消息 id）为非核心可空列：老版本缺列时整列降级 NULL
    let umid = opt_col(conn, "turn_usage", "user_message_id");
    let sql = format!(
        "SELECT session_id, turn_id, COALESCE(status, ''), started_at, {completed}, \
         {inp}, {out}, {rt}, {cw}, {cr}, {req}, {retry}, {tool}, {dur}, {ttft}, {umid} \
         FROM turn_usage \
         WHERE started_at >= ?1 AND session_id NOT LIKE 'sess_subagent%' \
         ORDER BY started_at ASC"
    );
    let mut stmt = conn
        .prepare(&sql)
        .map_err(|e| format!("准备 turn_usage 查询失败: {e}"))?;
    let mut turns: Vec<UsageTurn> = stmt
        .query_map([window_start_ms], |row| {
            Ok(UsageTurn {
                session_id: row.get(0)?,
                turn_id: row.get(1)?,
                status: row.get(2)?,
                start: row.get::<_, Option<i64>>(3)?.unwrap_or(0),
                end: row.get(4)?,
                input_tokens: row.get::<_, i64>(5)?.max(0),
                output_tokens: row.get::<_, i64>(6)?.max(0),
                reasoning: row.get::<_, i64>(7)?.max(0),
                cache_write: row.get::<_, i64>(8)?.max(0),
                cache_read: row.get::<_, i64>(9)?.max(0),
                requests: row.get::<_, i64>(10)?.max(0),
                retries: row.get::<_, i64>(11)?.max(0),
                tool_calls: row.get::<_, i64>(12)?.max(0),
                dur: row.get::<_, Option<i64>>(13)?,
                ttft: row.get::<_, Option<i64>>(14)?,
                user_message_id: row.get::<_, Option<String>>(15)?,
                sub: None,
                subagent: None,
                models: String::new(),
                speed: None,
            })
        })
        .map_err(|e| format!("读取 turn_usage 失败: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("读取 turn_usage 失败: {e}"))?;
    turns.retain(|t| t.start > 0 && !t.turn_id.is_empty());
    // 行数上限：超出保留最新（序列按 started_at 升序，丢弃头部最旧的轮次）
    if turns.len() > MAX_TURNS {
        turns.drain(..turns.len() - MAX_TURNS);
    }

    // ---- 子代理轮（并入父会话中时间覆盖它的主轮，见模块头验证结论）----
    let mut subs: Vec<SubTurnRow> = Vec::new();
    // 时间窗口比对需要两侧 completed_at；session 表/parent_id 列缺失
    // （老版本）时放弃子轮查询（既不并入主轮，也不产出 V20 自身视图行）
    if has_table(conn, "session")
        && crate::db::has_column(conn, "session", "parent_id")
        && crate::db::has_column(conn, "turn_usage", "completed_at")
    {
        let (s_inp, s_out, s_rt) = (
            num_col(conn, "turn_usage", "input_tokens"),
            num_col(conn, "turn_usage", "output_tokens"),
            num_col(conn, "turn_usage", "reasoning_tokens"),
        );
        let (s_cw, s_cr) = (
            num_col(conn, "turn_usage", "cache_creation_input_tokens"),
            num_col(conn, "turn_usage", "cache_read_input_tokens"),
        );
        let (s_req, s_retry, s_tool) = (
            num_col(conn, "turn_usage", "model_request_count"),
            num_col(conn, "turn_usage", "model_retry_count"),
            num_col(conn, "turn_usage", "tool_call_count"),
        );
        // 自身视图行口径的可空列：umid（DOM 匹配键）与 dur/ttft，列缺失
        // 时整列降级 NULL（老版本库按 null 透出，与主轮查询同款降级）
        let s_umid = opt_col(conn, "turn_usage", "user_message_id");
        let s_dur = opt_col(conn, "turn_usage", "duration_ms");
        let s_ttft = opt_col(conn, "turn_usage", "time_to_first_token_ms");
        let sub_sql = format!(
            "SELECT tu.turn_id, tu.session_id, s.parent_id, \
             COALESCE(tu.status, ''), tu.started_at, tu.completed_at, \
             {s_umid}, {s_dur}, {s_ttft}, \
             {s_inp}, {s_out}, {s_rt}, {s_cw}, {s_cr}, {s_req}, {s_retry}, {s_tool} \
             FROM turn_usage tu JOIN session s ON s.id = tu.session_id \
             WHERE tu.started_at >= ?1 AND tu.session_id LIKE 'sess_subagent%'"
        );
        let mut stmt = conn
            .prepare(&sub_sql)
            .map_err(|e| format!("准备子代理轮查询失败: {e}"))?;
        let rows = stmt
            .query_map([window_start_ms], |row| {
                Ok(SubTurnRow {
                    turn_id: row.get(0)?,
                    session_id: row.get(1)?,
                    parent_session_id: row.get(2)?,
                    status: row.get(3)?,
                    started_at: row.get::<_, Option<i64>>(4)?.unwrap_or(0),
                    completed_at: row.get(5)?,
                    user_message_id: row.get(6)?,
                    dur: row.get(7)?,
                    ttft: row.get(8)?,
                    input_tokens: row.get::<_, i64>(9)?.max(0),
                    output_tokens: row.get::<_, i64>(10)?.max(0),
                    reasoning: row.get::<_, i64>(11)?.max(0),
                    cache_write: row.get::<_, i64>(12)?.max(0),
                    cache_read: row.get::<_, i64>(13)?.max(0),
                    requests: row.get::<_, i64>(14)?.max(0),
                    retries: row.get::<_, i64>(15)?.max(0),
                    tool_calls: row.get::<_, i64>(16)?.max(0),
                })
            })
            .map_err(|e| format!("读取子代理轮失败: {e}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("读取子代理轮失败: {e}"))?;
        subs.extend(rows);
    }
    // ---- 子代理自身视图行（V20）：每条已完成子代理轮除并入主轮外，
    //      额外以自身口径导出一条 turns 行（sess = 子代理会话 id、
    //      umid = 子轮自己的用户消息 id、数值/dur/ttft 均为该子轮自身
    //      值、带 subagent:1 标记——标记仅为数据侧自描述字段，前端
    //      匹配不读取该键：详情面板靠自身行 umid 进入完成索引（index）
    //      走完成态显示真实值——否则导出数据永远没有子代理会话自身
    //      的行，前端 findLiveNodes 把已完成子代理轮误判为活动轮，面板
    //      渲染全 0 启动窗口占位（90 秒后枯萎移除）。并入主轮的行照常
    //      并存导出（主轮行仍含并入的 sub 数值），孤儿/越界丢弃的子轮
    //      同样导出自身行（它们只是不并入主轮，面板显示不受取舍影响）。
    //      防双计：会话累计 sessionTotals 按 t.sess 精确匹配，主会话
    //      视图查主会话 id 不命中子代理自身行、子代理视图只命中自身行，
    //      各自恰计一次；runs 侧子代理行在该轮落库后即被 done 集合
    //      过滤，与自身行不同通道不重叠。旧渲染脚本忽略未知字段，
    //      导出协议 v 保持 2 向后兼容。----
    let self_view_rows: Vec<UsageTurn> = subs
        .iter()
        .filter(|s| s.started_at > 0 && !s.turn_id.is_empty())
        .map(|s| UsageTurn {
            turn_id: s.turn_id.clone(),
            user_message_id: s.user_message_id.clone(),
            session_id: s.session_id.clone(),
            status: s.status.clone(),
            start: s.started_at,
            end: s.completed_at,
            input_tokens: s.input_tokens,
            output_tokens: s.output_tokens,
            cache_read: s.cache_read,
            cache_write: s.cache_write,
            reasoning: s.reasoning,
            requests: s.requests,
            retries: s.retries,
            tool_calls: s.tool_calls,
            dur: s.dur,
            ttft: s.ttft,
            sub: None,
            subagent: Some(1),
            models: String::new(),
            speed: None,
        })
        .collect();
    let (merged_pairs, sub_orphans) = merge_subagent_turns(&mut turns, subs);
    // 自身视图行并入导出序列：重排保持 started_at 升序契约（截断语义
    // "丢最旧"依赖升序；稳定排序下同刻主轮行保持在自身行之前），再统
    // 一截断行数上限（主轮查询后的截断只限主轮序列，混合序列在此收口）
    turns.extend(self_view_rows);
    turns.sort_by_key(|t| t.start);
    if turns.len() > MAX_TURNS {
        turns.drain(..turns.len() - MAX_TURNS);
    }

    // ---- 模型清单：该轮 model_usage 的去重 model_id（含并入子轮）。
    //      CLI 后台请求（session_title/goal_summary_title 等）的行 turn_id
    //      为 NULL，而 SELECT 首列按 String 强转不容忍 NULL，必须在 SQL
    //      排除，否则一行脏数据即中断整轮导出（usage-data.js 停更）----
    if has_table(conn, "model_usage")
        && crate::db::has_column(conn, "model_usage", "turn_id")
        && crate::db::has_column(conn, "model_usage", "model_id")
        && crate::db::has_column(conn, "model_usage", "started_at")
    {
        let mut stmt = conn
            .prepare(
                "SELECT turn_id, model_id FROM model_usage \
                 WHERE started_at >= ?1 AND turn_id IS NOT NULL \
                 AND model_id IS NOT NULL AND model_id != ''",
            )
            .map_err(|e| format!("准备 model_usage 查询失败: {e}"))?;
        let rows = stmt
            .query_map([window_start_ms], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(|e| format!("读取 model_usage 失败: {e}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("读取 model_usage 失败: {e}"))?;
        attach_models(&mut turns, &build_models_map(rows), &merged_pairs);
    }

    Ok(Some((turns, sub_orphans)))
}

/// 读出扫查窗口内已完成轮的 turn_id 集合（turn_usage 有行 = 整轮已结束）。
/// runs 据此过滤完成轮；窗口取完整 7 天而非 10 分钟——长轮（>10 分钟）的
/// turn_usage.started_at 早于 runs 新鲜度窗口，漏查会导致完成后最长 10 分钟
/// 内残留 runs 行（渲染端虽以 turn_usage 优先，但会话累计会双重计数）。
pub(crate) fn collect_done_turn_ids(
    conn: &Connection,
    sweep_start_ms: i64,
) -> Result<BTreeSet<String>, String> {
    let mut stmt = conn
        .prepare("SELECT DISTINCT turn_id FROM turn_usage WHERE started_at >= ?1")
        .map_err(|e| format!("准备完成轮查询失败: {e}"))?;
    let rows = stmt
        .query_map([sweep_start_ms], |row| row.get::<_, String>(0))
        .map_err(|e| format!("读取完成轮失败: {e}"))?
        .collect::<Result<BTreeSet<_>, _>>()
        .map_err(|e| format!("读取完成轮失败: {e}"))?;
    Ok(rows)
}

/// 读出 pu（待处理用户消息）时刻：最近一条「尚无对应完成轮」的 user 消息
/// 的 time_created（毫秒）。方案与实测依据见模块头 pu 信号说明——rowid
/// 尾部 PENDING_SCAN_ROWS 行索引序扫 + json_extract 解析角色（实测
/// 0.023ms@10258 行主库，成本与表大小无关，绝不全表扫）；完成轮匹配与
/// 调用方已聚合的 turns umid 集合（turn_usage.user_message_id，含子代理
/// 自身视图行）内存比对，不回查库（该列无索引，回查即 turn_usage 全表
/// 扫）。返回按 time_created 降序的第一条未匹配且满足新鲜期/活跃佐证的
/// user 消息（即最近的待处理消息；更早的未匹配消息不透出——陈旧异常消
/// 息由消费端 90 秒窗口兜底，不放大信号）。
/// A1 新鲜期与活跃佐证：`now_ms − time_created ≤ PENDING_FRESH_MS` 内的
/// 消息可独立启动"等待首请求"；超期消息只有其会话（或会话树根）出现在
/// `corroborating_sessions`（调用方传入的进行中轮所在会话及其树根集合）
/// 时才继续保留——真实耗时很长的首请求一旦落库首行请求即转入 runs 通
/// 道，佐证随之成立；无任何活跃证据的超期孤儿消息按无待处理处理，不再
/// 永久冒充活跃轮。取消/失败轮落 turn_usage 行（umid 进完成集合）与新
/// 一轮 user 消息（更新者胜出）都会立即清掉旧等待。
/// - Ok(None) = 无待处理消息，或 message 表/核心列缺失（老版本库，
///   pu 信号整体缺失，宠物端退化为既有行为）；
/// - Err = 瞬态查询失败（调用方静默降级为无信号，不阻塞导出）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PendingUser {
    pub(crate) id: String,
    pub(crate) session_id: String,
    pub(crate) time_created: i64,
}

/// Full pending-user signal. The session id is retained because the first
/// model request may not have produced a turn/run yet; session statistics still
/// need this id to load the existing model_usage lifetime total.
pub(crate) fn collect_pending_user(
    conn: &Connection,
    done_user_msg_ids: &BTreeSet<String>,
    now_ms: i64,
    corroborating_sessions: &BTreeSet<String>,
) -> Result<Option<PendingUser>, String> {
    // 核心列探测降级与 collect_turns 同款：表/核心列缺失 → 无 pu 信号
    if !has_table(conn, "message")
        || !crate::db::has_column(conn, "message", "id")
        || !crate::db::has_column(conn, "message", "time_created")
        || !crate::db::has_column(conn, "message", "data")
    {
        return Ok(None);
    }
    let mut stmt = conn
        .prepare(
            "SELECT id, session_id, time_created FROM \
             (SELECT id, session_id, time_created, json_extract(data, '$.role') AS role \
              FROM message ORDER BY rowid DESC LIMIT ?1) \
             WHERE role = 'user' ORDER BY time_created DESC",
        )
        .map_err(|e| format!("准备待处理用户消息查询失败: {e}"))?;
    let rows = stmt
        .query_map([PENDING_SCAN_ROWS], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<i64>>(2)?,
                ))
            })
        .map_err(|e| format!("读取待处理用户消息失败: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("读取待处理用户消息失败: {e}"))?;
    for (id, session_id, tc) in rows {
        if done_user_msg_ids.contains(&id) {
            continue;
        }
        // 脏行防御：无效时刻不透出（与 turns 的 start > 0 口径一致）
        if let Some(t) = tc.filter(|value| *value > 0) {
            if session_id.is_empty() {
                continue;
            }
            // A1：新鲜期内独立成立；超期必须有会话树内的活跃佐证
            let fresh = now_ms.saturating_sub(t) <= PENDING_FRESH_MS;
            let corroborated = corroborating_sessions.contains(&session_id);
            if fresh || corroborated {
                return Ok(Some(PendingUser {
                    id,
                    session_id,
                    time_created: t,
                }));
            }
        }
    }
    Ok(None)
}

/// Compatibility projection used by the pet state machine, which only needs
/// the timestamp. Callers that build session statistics should use
/// `collect_pending_user` so the session id is not lost.
pub(crate) fn collect_pending_user_ms(
    conn: &Connection,
    done_user_msg_ids: &BTreeSet<String>,
    now_ms: i64,
    corroborating_sessions: &BTreeSet<String>,
) -> Result<Option<i64>, String> {
    Ok(collect_pending_user(conn, done_user_msg_ids, now_ms, corroborating_sessions)?
        .map(|pending| pending.time_created))
}

/// 读出 ta（活跃工具）时刻：最新一条 running 状态工具行的 started_at
/// （毫秒）。工具调用开始瞬间即落库（status='running'、completed_at 为
/// NULL），完成时更新为 completed——是"正在执行工具"的实时信号（比
/// model_usage 的请求完成落库强）。查询走 tool_usage_started_tool_idx
/// 索引（started_at 前缀）+ MAX 聚合，实测 10805 行主库平均 0.003ms；
/// 窗口兜底崩溃残留的 running 行（见 TOOL_WINDOW_MS）。
/// - Ok(None) = 窗口内无 running 行，或 tool_usage 表/核心列缺失
///   （老版本库，ta 信号整体缺失，宠物端退化为既有行为）；
/// - Err = 瞬态查询失败（调用方静默降级为无信号，不阻塞导出）。
pub(crate) fn collect_active_tool_ms(
    conn: &Connection,
    window_start_ms: i64,
) -> Result<Option<i64>, String> {
    if !has_table(conn, "tool_usage")
        || !crate::db::has_column(conn, "tool_usage", "status")
        || !crate::db::has_column(conn, "tool_usage", "started_at")
    {
        return Ok(None);
    }
    conn.query_row(
        "SELECT MAX(started_at) FROM tool_usage \
         WHERE status = 'running' AND started_at >= ?1",
        [window_start_ms],
        |row| row.get::<_, Option<i64>>(0),
    )
    .map(|v| v.filter(|t| *t > 0))
    .map_err(|e| format!("读取活跃工具失败: {e}"))
}

/// 读出 fe（失败轮事件）时刻：窗口内最近一次「失败或取消」完成轮的
/// completed_at（毫秒）。判定 = `status != 'completed' || cancelled_by_user
/// = 1 || tool_error_count > 0`（status 为 cancelled/error，或轮内含用户
/// 取消/工具报错——成功轮不命中条件，MAX 聚合天然满足"fe 只在失败轮
/// 新增时才变化"，成功轮完成不刷新 fe）。实测 334 行主库平均 0.018ms
/// （turn_usage 行数量级为每天几十行，窗口过滤 + MAX 无压力）。
/// - Ok(None) = 窗口内无失败轮（或最近失败轮的 completed_at 为 NULL
///   的异常中断行，无法定位失败时刻，不透出），或 turn_usage 表/
///   completed_at 列缺失（fe 信号整体缺失）；
/// - 判定列降级：cancelled_by_user / tool_error_count 缺列（老版本库）
///   时省略对应 OR 子句，仅按 status 判定（列存在时三者任一命中即失败）；
/// - Err = 瞬态查询失败（调用方静默降级为无信号，不阻塞导出）。
pub(crate) fn collect_failure_event_ms(
    conn: &Connection,
    window_start_ms: i64,
) -> Result<Option<i64>, String> {
    if !has_table(conn, "turn_usage")
        || !crate::db::has_column(conn, "turn_usage", "status")
        || !crate::db::has_column(conn, "turn_usage", "started_at")
        || !crate::db::has_column(conn, "turn_usage", "completed_at")
    {
        return Ok(None);
    }
    let mut cond = String::from("COALESCE(status, '') != 'completed'");
    if crate::db::has_column(conn, "turn_usage", "cancelled_by_user") {
        cond.push_str(" OR COALESCE(cancelled_by_user, 0) = 1");
    }
    if crate::db::has_column(conn, "turn_usage", "tool_error_count") {
        cond.push_str(" OR COALESCE(tool_error_count, 0) > 0");
    }
    let sql = format!(
        "SELECT MAX(completed_at) FROM turn_usage WHERE started_at >= ?1 AND ({cond})"
    );
    conn.query_row(&sql, [window_start_ms], |row| row.get::<_, Option<i64>>(0))
        .map(|v| v.filter(|t| *t > 0))
        .map_err(|e| format!("读取失败轮事件失败: {e}"))
}

/// 聚合进行中轮（runs）：近 10 分钟有模型请求、且 turn_usage 尚无该
/// turn_id 行的轮，按 turn_id 分组输出 model_usage 合计。
/// - recent_start_ms = now − 10 分钟（新鲜度窗口）；sweep_start_ms =
///   now − 7 天（组行扫查下界，覆盖长轮早期请求的完整聚合）；
/// - 父会话保活（V20）：主会话行额外放宽为"该轮近 10 分钟有自身请求，
///   或名下有子代理会话近 10 分钟在产生请求"（防主轮派发子代理后静默
///   等待满 10 分钟被踢出 runs，见下方 keepalive 分支注释）；子代理行
///   与无子代理保活的僵尸主轮仍维持 10 分钟新鲜度窗口；
/// - sub_orphans = 游离子代理完成轮（collect_turns/merge 阶段分流输出，
///   所属主轮尚未落库的部分）：按 parent_session_id 并入对应主会话行
///   sub（与子代理 runs 行同口径聚合）；
/// - 每条主会话行 sub = 同批内 psess 指向它的子代理行 + 游离子代理
///   完成轮（多子代理并行全并）；父会话存在主会话行的子代理行打 m:1
///   （防会话累计双计，见模块头 runs 侧并入策略）；
/// - model_usage 表/核心列缺失（老版本库）→ Ok(空)，不影响 turns 导出；
/// - 行数量级：窗口内行数 = 10 分钟内完成的模型请求数（重度使用数百行），
///   分组后 runs 行数 = 活跃轮数（通常个位数），每 2 秒一次开销可忽略。
/// Public compatibility wrapper used by the pet worker. The pet contract does
/// not contain speed fields, so this path deliberately returns count-only rows.
/// The main usage export calls `collect_runs_raw` and attaches speeds from its
/// shared catalog once.
pub(crate) fn collect_runs(
    conn: &Connection,
    recent_start_ms: i64,
    sweep_start_ms: i64,
    done_turn_ids: &BTreeSet<String>,
    sub_orphans: &[SubTurnRow],
) -> Result<Vec<UsageRun>, String> {
    collect_runs_raw(
        conn,
        recent_start_ms,
        sweep_start_ms,
        done_turn_ids,
        sub_orphans,
    )
}

fn collect_runs_raw(
    conn: &Connection,
    recent_start_ms: i64,
    sweep_start_ms: i64,
    done_turn_ids: &BTreeSet<String>,
    sub_orphans: &[SubTurnRow],
) -> Result<Vec<UsageRun>, String> {
    // 核心列缺失 → 无 runs（老版本库无 parent_user_message_id 等列时
    // 按下方 num_col/opt_col 逐列降级，不整体放弃）
    if !has_table(conn, "model_usage")
        || !crate::db::has_column(conn, "model_usage", "turn_id")
        || !crate::db::has_column(conn, "model_usage", "session_id")
        || !crate::db::has_column(conn, "model_usage", "started_at")
    {
        return Ok(Vec::new());
    }
    let (inp, out, rt) = (
        num_col(conn, "model_usage", "input_tokens"),
        num_col(conn, "model_usage", "output_tokens"),
        num_col(conn, "model_usage", "reasoning_tokens"),
    );
    let (cw, cr) = (
        num_col(conn, "model_usage", "cache_creation_input_tokens"),
        num_col(conn, "model_usage", "cache_read_input_tokens"),
    );
    // umid 可空列：老版本缺列时整列降级 NULL（MAX 聚合对 NULL 行透明，
    // 组内同轮取值一致；全 NULL 组输出 NULL）
    let umid = opt_col(conn, "model_usage", "parent_user_message_id");
    let umid_expr = if umid == "NULL" {
        "NULL".to_string()
    } else {
        format!("MAX({umid})")
    };
    // psess 仅子代理会话有值：session 表/parent_id 缺失（老版本）时整列
    // 降级 NULL（放弃并入父会话，该 run 仅按 sess 命中子会话自身）
    let has_parent = has_table(conn, "session")
        && crate::db::has_column(conn, "session", "parent_id");
    let join = if has_parent {
        "LEFT JOIN session s ON s.id = mu.session_id"
    } else {
        ""
    };
    let psess_expr = if has_parent { "s.parent_id" } else { "NULL" };
    // V20 父会话保活分支（修复一）：主轮派发子代理后自身静默等待，满
    // 10 分钟会被下方新鲜度窗口整体踢出 runs——前端每轮条跌回全 0 启
    // 动窗口占位、会话累计丢失该轮实时值，且其子代理完成轮（孤儿）失
    // 去挂载点、消耗蒸发。主会话行放宽保留条件："该轮近 10 分钟有自身
    // 请求，或名下有子代理会话近 10 分钟在产生请求"（经 session.parent_id
    // 查父会话集合）。子代理行维持原 10 分钟窗口条件不变；无子代理保活
    // 的僵尸主轮（异常中断、永不落库的轮）仍在 10 分钟静默后退出 runs。
    // 会话级保活的已知边界：父会话保活期间，同会话更早的异常中断轮（同
    // 样未落库）会一并回流 runs，其数值计入会话累计直到保活消失后随窗
    // 口退出——正常中断会落 turn_usage（cancelled）进 done 集合被过滤，
    // 此场景仅限同会话叠加异常中断，属可接受的窄边缘。session 表缺
    // parent_id 列（老版本）时不拼入此分支，维持既有降级行为
    //（与 psess 同源判定）。
    let keepalive = if has_parent {
        " OR (mu.session_id NOT LIKE 'sess_subagent%' AND mu.session_id IN \
           (SELECT s2.parent_id FROM model_usage mu2 \
            JOIN session s2 ON s2.id = mu2.session_id \
            WHERE mu2.started_at >= ?1 AND mu2.session_id LIKE 'sess_subagent%' \
            AND s2.parent_id IS NOT NULL AND s2.parent_id != ''))"
    } else {
        ""
    };
    // 外层扫查限 7 天窗口（行数几万级可控），IN 子查询限定"近 10 分钟有
    // 请求"的轮——组内聚合含窗口外的早期请求（长轮完整合计）；保活
    // 分支按会话命中（turn_id 全局唯一，按 turn_id 分组后组行整体保留，
    // 组内聚合同样完整含窗口外早期请求）。mu.turn_id IS NOT NULL 防御：
    // CLI 后台请求（session_title/goal_summary_title 等）的行 turn_id 为
    // NULL，IN 子查询的三值逻辑天然挡住它们，但 keepalive 分支只按会话
    // 命中、不经过 turn_id——NULL 行一旦命中成组，SELECT 首列按 String
    // 强转即失败并中断整轮导出，必须在 SQL 层排除
    let sql = format!(
        "SELECT mu.turn_id, {umid_expr}, mu.session_id, {psess_expr}, \
         SUM({inp}), SUM({out}), SUM({cr}), SUM({cw}), SUM({rt}), COUNT(*), \
         MIN(mu.started_at) \
         FROM model_usage mu {join} \
         WHERE mu.started_at >= ?2 AND mu.turn_id IS NOT NULL AND (mu.turn_id IN \
           (SELECT turn_id FROM model_usage WHERE started_at >= ?1){keepalive}) \
         GROUP BY mu.turn_id, mu.session_id"
    );
    let mut stmt = conn
        .prepare(&sql)
        .map_err(|e| format!("准备进行中轮查询失败: {e}"))?;
    let rows = stmt
        .query_map(rusqlite::params![recent_start_ms, sweep_start_ms], |row| {
            let turn_id: String = row.get(0)?;
            Ok((
                turn_id.clone(),
                UsageRun {
                    turn_id,
                    user_message_id: row.get(1)?,
                    session_id: row.get::<_, Option<String>>(2)?.unwrap_or_default(),
                    parent_session_id: row.get(3)?,
                    input_tokens: row.get::<_, Option<i64>>(4)?.unwrap_or(0).max(0),
                    output_tokens: row.get::<_, Option<i64>>(5)?.unwrap_or(0).max(0),
                    cache_read: row.get::<_, Option<i64>>(6)?.unwrap_or(0).max(0),
                    cache_write: row.get::<_, Option<i64>>(7)?.unwrap_or(0).max(0),
                    reasoning: row.get::<_, Option<i64>>(8)?.unwrap_or(0).max(0),
                    requests: row.get::<_, i64>(9)?.max(0),
                    start: row.get::<_, Option<i64>>(10)?.unwrap_or(0),
                    merged: None,
                    sub: None,
                    speed: None,
                },
            ))
        })
        .map_err(|e| format!("读取进行中轮失败: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("读取进行中轮失败: {e}"))?;
    let mut runs = Vec::new();
    for (turn_id, mut run) in rows {
        // 完成轮不走此通道（turn_usage 已有行的 turn_id 直接丢弃）
        if done_turn_ids.contains(&turn_id) {
            continue;
        }
        // 脏行防御：无会话 id / 无有效开始时刻的行不导出（与 turns 的
        // start > 0 口径一致）
        if run.session_id.is_empty() || run.start <= 0 {
            continue;
        }
        // psess 仅子代理会话携带（主会话 parent_id 理论为 NULL，此处按
        // 前缀强制归 None，与 turns 导出的 sess_subagent% 口径一致）
        if !run.session_id.starts_with("sess_subagent") {
            run.parent_session_id = None;
        }
        runs.push(run);
    }
    merge_sub_runs(&mut runs, sub_orphans);
    Ok(runs)
}

/// runs 侧子代理并入（V9）：把子代理实时消耗并进主会话行 sub，并给
/// 已并入的子代理行打 m:1 防双计标记。
/// - sub 聚合 = 同批内 psess 指向该主会话的子代理行（并行多子代理全并）
///   + 游离子代理完成轮（按 parent_session_id 归并，collect_turns/merge
///   阶段已防与 turns 侧整轮并入重复）；
/// - m:1 仅在父会话存在进行中主轮行时置位（其数值确已并入该行 sub）；
///   父会话暂无主轮行的子代理行不打标，由渲染端按 psess 直接并入会话
///   累计，主轮行出现后自动切换口径（sub 与 m 同批一致，无缝衔接）。
fn merge_sub_runs(runs: &mut [UsageRun], sub_orphans: &[SubTurnRow]) {
    // psess → 子代理实时聚合（子代理 runs 行与游离子代理完成轮同池）
    let mut agg: BTreeMap<String, SubAgg> = BTreeMap::new();
    for r in runs.iter() {
        if !r.session_id.starts_with("sess_subagent") {
            continue;
        }
        let Some(p) = r.parent_session_id.as_deref() else {
            continue;
        };
        let e = agg.entry(p.to_string()).or_insert_with(SubAgg::empty);
        e.add(
            r.requests,
            r.input_tokens,
            r.output_tokens,
            r.cache_read,
            r.cache_write,
            r.reasoning,
        );
    }
    for o in sub_orphans {
        let Some(p) = o.parent_session_id.as_deref() else {
            continue;
        };
        let e = agg.entry(p.to_string()).or_insert_with(SubAgg::empty);
        e.add(
            o.requests,
            o.input_tokens,
            o.output_tokens,
            o.cache_read,
            o.cache_write,
            o.reasoning,
        );
    }
    // 主会话行消费聚合；子代理行按"父会话存在主会话行"打 m:1
    let main_sessions: BTreeSet<String> = runs
        .iter()
        .filter(|r| !r.session_id.starts_with("sess_subagent"))
        .map(|r| r.session_id.clone())
        .collect();
    for r in runs.iter_mut() {
        if r.session_id.starts_with("sess_subagent") {
            if r
                .parent_session_id
                .as_deref()
                .is_some_and(|p| main_sessions.contains(p))
            {
                r.merged = Some(1);
            }
        } else if let Some(s) = agg.get(r.session_id.as_str()) {
            r.sub = Some(s.clone());
        }
    }
}

/// 子代理轮并入：按"父会话 + 时间覆盖"（主轮 start ≤ 子轮 start 且
/// 子轮 end ≤ 主轮 end，见模块头）匹配主轮后累加 token 与次数；
/// dur/ttft 保持主轮自身口径不变。返回 (并入明细对, 游离子代理完成轮)：
/// 未并入且"所属主轮尚未落库"的子轮不再直接丢弃，而是输出给 runs 侧
/// 聚合进主会话行 sub（V9，主轮进行期间实时反映子代理消耗）。防双计
/// 判定沿用本函数的时间窗口匹配逻辑：父会话存在"start ≤ 子轮 start <
/// end"的完成主轮 = 子轮开始时所属主轮已在运行且已落库（子轮越界完成
/// 等窗口不匹配边界），维持丢弃不输出；仅当父会话没有任何覆盖子轮开始
/// 时刻的完成主轮（即所属主轮未落库、不在 turns）时才作为游离子代理
/// 完成轮输出。
fn merge_subagent_turns(
    turns: &mut [UsageTurn],
    subs: Vec<SubTurnRow>,
) -> (Vec<(String, String)>, Vec<SubTurnRow>) {
    let mut merged: Vec<(String, String)> = Vec::new();
    let mut orphans: Vec<SubTurnRow> = Vec::new();
    for sub in subs {
        // 无完成时刻或无父会话的子轮无法做时间窗口匹配，直接丢弃
        let (Some(sub_end), Some(parent)) =
            (sub.completed_at, sub.parent_session_id.clone())
        else {
            continue;
        };
        let Some(t) = turns.iter_mut().find(|t| {
            t.session_id == parent && t.start <= sub.started_at && t.end.is_some_and(|e| sub_end <= e)
        }) else {
            // 未命中：仅"所属主轮未落库（不在 turns）"时输出给 runs 侧
            let owner_done = turns.iter().any(|t| {
                t.session_id == parent
                    && t.start <= sub.started_at
                    && t.end.is_some_and(|e| sub.started_at < e)
            });
            if !owner_done {
                orphans.push(sub);
            }
            continue;
        };
        t.input_tokens += sub.input_tokens;
        t.output_tokens += sub.output_tokens;
        t.cache_read += sub.cache_read;
        t.cache_write += sub.cache_write;
        t.reasoning += sub.reasoning;
        t.requests += sub.requests;
        t.retries += sub.retries;
        t.tool_calls += sub.tool_calls;
        let s = t.sub.get_or_insert_with(SubAgg::empty);
        s.add(
            sub.requests,
            sub.input_tokens,
            sub.output_tokens,
            sub.cache_read,
            sub.cache_write,
            sub.reasoning,
        );
        merged.push((t.turn_id.clone(), sub.turn_id));
    }
    (merged, orphans)
}

/// turn_id → 去重模型清单（保持首次出现顺序）
fn build_models_map(rows: Vec<(String, String)>) -> BTreeMap<String, Vec<String>> {
    let mut map: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (turn_id, model) in rows {
        let list = map.entry(turn_id).or_default();
        if !list.contains(&model) {
            list.push(model);
        }
    }
    map
}

/// 把模型清单拼进各主轮：主轮自身模型 + 并入子轮模型，去重后逗号拼接。
fn attach_models(
    turns: &mut [UsageTurn],
    models: &BTreeMap<String, Vec<String>>,
    merged_pairs: &[(String, String)],
) {
    for t in turns.iter_mut() {
        let mut list = models.get(&t.turn_id).cloned().unwrap_or_default();
        for (main_id, sub_id) in merged_pairs {
            if main_id == &t.turn_id {
                if let Some(sub_models) = models.get(sub_id) {
                    for m in sub_models {
                        if !list.contains(m) {
                            list.push(m.clone());
                        }
                    }
                }
            }
        }
        t.models = list.join(",");
    }
}

// ============================================================
// 会话级导出（sess 数组：model_usage 全量合计）
// ============================================================

/// 会话级统计行（sess 数组元素，v2 附加字段，旧渲染脚本忽略未知字段）。
/// 键名即 usage.js 消费端契约，勿改。内容为会话树内 model_usage 逐笔
/// 累加的**全量合计**（tt/up/down/cr/rq，含失败/中断轮——turn_usage 覆盖
/// 不全，轮正常结束才落库，按轮聚合的旧口径会话累计系统性偏小，本通道
/// 修正为请求级全量）；↑ 口径 = 逐笔 max(0, input − cache_read)（与
/// 悬浮窗 Σ/注入版会话条一致）。
/// 历史注记：V21 曾携带 CTX 上下文占用三字段（cp/cu/cw，树内最近一笔
/// completed 请求的 input ÷ 窗口容量），V22 随展示下线一并删除（数据端
/// 不再查询/序列化，context_window 模块随之移除）。
#[derive(Debug, Clone, Serialize)]
pub(crate) struct UsageSessionStat {
    /// 会话 id（主会话行为"会话树"口径——含全部子代理归并；子代理会话
    /// 若自身出现在 turns/runs 亦导出"自身树"行供其详情面板查询）
    #[serde(rename = "s")]
    session_id: String,
    /// 全量合计 Σ = ↑+↓+⟲（model_usage 逐笔，含失败轮）
    #[serde(rename = "tt")]
    total: i64,
    /// 全量合计 ↑ 非缓存输入（逐笔 clamp）
    #[serde(rename = "up")]
    plain_in: i64,
    /// 全量合计 ↓ 输出
    #[serde(rename = "down")]
    out_tokens: i64,
    /// 全量合计 ⟲ 缓存读
    #[serde(rename = "cr")]
    cache_read: i64,
    /// 全量合计 × 请求笔数（model_usage 行数）
    #[serde(rename = "rq")]
    requests: i64,
    /// 会话树内最新模型请求的速度。
    #[serde(skip_serializing_if = "Option::is_none")]
    speed: Option<SpeedSnapshot>,
    /// 当前会话树的速度状态；进行中且没有本轮确认请求时为 measuring。
    #[serde(rename = "speedState")]
    speed_state: SpeedState,
    /// Internal only: members used to calculate the root session's lifetime
    /// total and latest speed. The small speed side channel needs this after
    /// the full history export, but it is not part of either JS contract.
    #[serde(skip)]
    member_session_ids: Vec<String>,
    /// Internal only: current pending user-message boundary for this tree.
    /// It prevents a one-second refresh from restoring the previous round's
    /// session speed before the new request has produced a completed row.
    #[serde(skip)]
    round_started_at: Option<i64>,
    /// Internal only: 独立的"该会话树当前确有轮在运行"布尔量（A2）——由
    /// 进行中轮或新鲜待处理用户消息推导，绝不从 speed_state 反推。旁路
    /// 的 1 秒刷新据此在"速度暂时为空"时回落 Measuring 而不是 Unavailable。
    #[serde(skip)]
    generating: bool,
}

/// 单成员会话的 model_usage 聚合中间值（全量合计用）
#[derive(Default, Clone, Copy)]
struct SessionUsageAgg {
    plain_in: i64,
    out_tokens: i64,
    cache_read: i64,
    requests: i64,
}

impl SessionUsageAgg {
    fn total(&self) -> i64 {
        self.plain_in + self.out_tokens + self.cache_read
    }
}

/// 收集会话级统计（sess 数组数据源，model_usage 全量合计）：
/// - 目标会话 = turns/runs 中出现过的全部会话 id ∪ 各自沿 parent_id 上溯
///   到的根会话（子代理活动已归并的主会话即使自身未出现也导出，渲染端
///   会话条按主会话 id 查询必命中）；每个目标导出"以它为根的会话树"
///   （自身 + 全部后代子代理）合计——主会话行天然含子代理（V9 归并
///   口径），子代理行 = 自身及更深层后代；
/// - 全量合计走一次 `session_id IN (...)` 索引等值查（成员并集一次
///   查询、内存按树归并），无全表扫；
/// - session 表/parent_id 缺失（老版本库）降级为无归并：每个出现的会话
///   id 仅聚合自身；
/// - 查询失败返回 Err（调用方按附加通道降级为空 sess，不阻塞 turns/runs
///   导出——渲染端对无 sess 数据回退旧口径）。
pub(crate) fn collect_session_stats(
    conn: &Connection,
    turns: &[UsageTurn],
    runs: &[UsageRun],
) -> Result<Vec<UsageSessionStat>, String> {
    let session_ids = session_ids_for_stats(conn, turns, runs, None)?;
    let catalog = SpeedCatalog::load(conn, &session_ids)?;
    collect_session_stats_with_catalog(conn, turns, runs, &catalog, None)
}

/// Session-statistics implementation used by the full export. `pending_user`
/// keeps the currently active session visible before its first model_usage row
/// has produced a run/turn and supplies the current-round lower bound, while
/// the catalog is shared with turns and runs.
fn collect_session_stats_with_catalog(
    conn: &Connection,
    turns: &[UsageTurn],
    runs: &[UsageRun],
    catalog: &SpeedCatalog,
    pending_user: Option<&PendingUser>,
) -> Result<Vec<UsageSessionStat>, String> {
    // 1) 出现过的会话集合（runs 含子代理进行中行，turns 含自身视图行）
    let mut appeared: BTreeSet<String> = BTreeSet::new();
    for t in turns {
        appeared.insert(t.session_id.clone());
    }
    for r in runs {
        appeared.insert(r.session_id.clone());
    }
    if let Some(pending) = pending_user.filter(|pending| !pending.session_id.is_empty()) {
        appeared.insert(pending.session_id.clone());
    }
    if appeared.is_empty() {
        return Ok(Vec::new());
    }

    // 2) 会话父子关系由共享索引一次加载：所有出口使用同一棵任意深度
    //    会话树，并由索引内部负责缺列降级与环路保护。
    let tree_index = crate::token_speed::load_session_tree_index(conn)?;

    // 3) 目标会话 = 出现集合 ∪ 各自的根。
    let mut targets: BTreeSet<String> = appeared.clone();
    if tree_index.has_parent() {
        for id in &appeared {
            targets.insert(tree_index.root_for(id));
        }
    }

    // 4) 各目标的树成员（BFS 收集全部后代；无 parent 列时成员即自身）
    let mut members_by_target: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut all_members: BTreeSet<String> = BTreeSet::new();
    for t in &targets {
        let members = tree_index.members(t);
        all_members.extend(members.iter().cloned());
        members_by_target.insert(t.clone(), members);
    }

    // 5) 全量合计：成员并集一次 IN 等值查（走 session_turn 前缀索引），
    //    逐笔 clamp 的 ↑ 在 SQL 内完成（SQLite 双参 MAX 为标量函数）
    let member_list: Vec<String> = all_members.into_iter().collect();
    let (inp, out, cr) = (
        num_col(conn, "model_usage", "input_tokens"),
        num_col(conn, "model_usage", "output_tokens"),
        num_col(conn, "model_usage", "cache_read_input_tokens"),
    );
    let placeholders = vec!["?"; member_list.len()].join(", ");
    let agg_sql = format!(
        "SELECT session_id, \
                SUM(MAX({inp} - {cr}, 0)), SUM({out}), SUM({cr}), COUNT(*) \
         FROM model_usage WHERE session_id IN ({placeholders}) \
         GROUP BY session_id"
    );
    let mut stmt = conn
        .prepare(&agg_sql)
        .map_err(|e| format!("准备会话合计查询失败: {e}"))?;
    let agg_rows = stmt
        .query_map(rusqlite::params_from_iter(member_list.iter()), |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, i64>(4)?,
            ))
        })
        .map_err(|e| format!("查询会话合计失败: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("读取会话合计失败: {e}"))?;
    let agg_by_session: BTreeMap<String, SessionUsageAgg> = agg_rows
        .into_iter()
        .map(|(sid, up, out, cr, rq)| {
            (
                sid,
                SessionUsageAgg {
                    plain_in: up.max(0),
                    out_tokens: out.max(0),
                    cache_read: cr.max(0),
                    requests: rq.max(0),
                },
            )
        })
        .collect();
    let now_ms = chrono::Utc::now().timestamp_millis();

    // 6) 按目标组装（树内逐成员累加全量合计）。CTX 查询（每成员最近一笔
    //    completed 请求 + 窗口容量解析）已随 V22 展示下线一并删除
    let mut out = Vec::with_capacity(targets.len());
    for (target, members) in &members_by_target {
        let mut agg = SessionUsageAgg::default();
        for m in members {
            if let Some(a) = agg_by_session.get(m) {
                agg.plain_in += a.plain_in;
                agg.out_tokens += a.out_tokens;
                agg.cache_read += a.cache_read;
                agg.requests += a.requests;
            }
        }
        let round_started_at = pending_user
            .filter(|pending| members.iter().any(|member| member == &pending.session_id))
            .map(|pending| pending.time_created);
        let latest = catalog.latest_for_members(members, round_started_at);
        let speed = speed_for_latest(latest, now_ms);
        let current_runs: Vec<&UsageRun> = runs
            .iter()
            .filter(|run| {
                members.contains(&run.session_id)
                    && round_started_at.is_none_or(|started| run.start >= started)
            })
            .collect();
        // A2：生成态是独立布尔量（本轮有进行中请求，或刚发出待处理用户
        // 消息），不从"有没有速度"反推；状态机允许"正在生成 + 本轮已完
        // 成请求有最近速度"（generating=true 且 Recent 同时成立）
        let generating = !current_runs.is_empty() || round_started_at.is_some();
        let speed_state = if speed.is_some() {
            SpeedState::Recent
        } else if generating {
            SpeedState::Measuring
        } else {
            SpeedState::Unavailable
        };
        out.push(UsageSessionStat {
            session_id: target.clone(),
            total: agg.total(),
            plain_in: agg.plain_in,
            out_tokens: agg.out_tokens,
            cache_read: agg.cache_read,
            requests: agg.requests,
            speed,
            speed_state,
            member_session_ids: members.clone(),
            round_started_at,
            generating,
        });
    }
    Ok(out)
}

// ============================================================
// 序列化与原子写出
// ============================================================

/// 数据快照内可见的最后活动时刻（毫秒，纯函数）：全部完成轮的 end、
/// 全部进行中轮的 start、待处理用户消息时刻 pu 与活跃工具时刻 ta 的
/// 最大值。口径与用途见模块头 la 字段说明（进行中轮以首请求时刻近似
/// "最新请求时刻"，runs 非空时状态机短路不消费 la，近似无影响；闲置
/// 判定实际只消费完成轮的精确 end）。pu 参与取大（V5）：用户发消息
/// 本身就是活动，pu 出现时宠物预判进 working，90 秒预判窗口结束后 la
/// 继续支撑闲置档（IDLE_SLEEP_MS 内 idle 而非直接入睡），与完成轮
/// end 的口径一致。ta 参与取大（V6）：工具开始也是活动，工具结束后
/// 30 秒 ta 仍托底闲置档（同 pu 语义；工具执行期间状态机短路进
/// tool_running 不消费 la，无影响）。
pub(crate) fn last_activity_ms(
    turns: &[UsageTurn],
    runs: &[UsageRun],
    pending_user: Option<i64>,
    active_tool: Option<i64>,
) -> i64 {
    let mut la = 0i64;
    for t in turns {
        if let Some(end) = t.end {
            la = la.max(end);
        }
    }
    for r in runs {
        la = la.max(r.start);
    }
    if let Some(pu) = pending_user {
        la = la.max(pu);
    }
    if let Some(ta) = active_tool {
        la = la.max(ta);
    }
    la
}

/// 渲染 usage-data.js 完整内容。v 为数据契约版本（v2 起含 umid 字段，
/// 渲染端对 v !== 2 视为无效数据走静默路径）；runs 为进行中轮附加字段
/// （v2 格式不变，旧渲染脚本忽略未知字段平滑兼容，空数组也输出）；
/// ts 为"最后数据变化时刻"（内容无变化跳写时文件保持上次的 ts，语义
/// 即最后一次数据变化的时刻，渲染端据此跳过无变化的重建与重渲染）；
/// la 为最后活动时刻（V2 附加字段，宠物闲置判定消费，见模块头）；
/// pu 为待处理用户消息时刻（V5 附加字段，null = 无待处理消息，宠物
/// 预判通道的消费键，见模块头 pu 信号说明）；ta/fe 为活跃工具与失败
/// 轮事件时刻（V6 附加字段，null = 无信号，宠物新状态通道的消费键，
/// 见模块头 ta/fe 信号说明；旧渲染脚本按未知字段忽略）。
fn render_usage_js(
    ts_ms: i64,
    la_ms: i64,
    pending_user: Option<i64>,
    active_tool: Option<i64>,
    failure_event: Option<i64>,
    turns_json: &str,
    runs_json: &str,
    sess_json: &str,
) -> String {
    let opt = |v: Option<i64>| match v {
        Some(t) => t.to_string(),
        None => "null".to_string(),
    };
    format!(
        "window.__ZBAR_USAGE__ = {{\"v\":2,\"ts\":{ts_ms},\"la\":{la_ms},\"pu\":{},\"ta\":{},\"fe\":{},\"turns\":{turns_json},\"runs\":{runs_json},\"sess\":{sess_json}}};\n",
        opt(pending_user),
        opt(active_tool),
        opt(failure_event)
    )
}

/// Render the volatile speed side channel. It contains only stable identity
/// keys plus a snapshot/null, never token counters or historical turn fields.
/// A3：历史轮不进旁路（`turns` 恒为空数组，保留键以维持 v1 文件形态，
/// 旧注入脚本按空数组合并无副作用）；稳定的历史轮速度随 2 秒大文件
/// 发布。
fn render_speed_js(
    ts_ms: i64,
    runs: &[SpeedRunEntry],
    sessions: &[SpeedSessionEntry],
) -> Result<String, String> {
    let runs_json = serde_json::to_string(runs)
        .map_err(|e| format!("序列化速度进行中快照失败: {e}"))?;
    let sessions_json = serde_json::to_string(sessions)
        .map_err(|e| format!("序列化速度会话快照失败: {e}"))?;
    Ok(format!(
        "window.__ZBAR_USAGE_SPEED__ = {{\"v\":1,\"ts\":{ts_ms},\"turns\":[],\"runs\":{runs_json},\"sess\":{sessions_json}}};\n"
    ))
}

/// Atomic write for the small speed file. The cache compares only its payload
/// arrays, so a one-second tick with no new/changed request does not write at
/// all; timestamp changes alone never cause disk churn.
fn write_speed_view(
    cache: &mut Option<String>,
    dir: &Path,
    view: &SpeedView,
    ts_ms: i64,
) -> Result<bool, String> {
    let runs_json = serde_json::to_string(&view.runs)
        .map_err(|e| format!("序列化速度进行中快照失败: {e}"))?;
    let sess_json = serde_json::to_string(&view.sessions)
        .map_err(|e| format!("序列化速度会话快照失败: {e}"))?;
    let mut payload = String::with_capacity(runs_json.len() + sess_json.len() + 2);
    payload.push_str(&runs_json);
    payload.push('\u{1}');
    payload.push_str(&sess_json);
    if cache.as_deref() == Some(payload.as_str()) {
        return Ok(false);
    }
    let target = dir.join(store::USAGE_SPEED_FILE);
    let tmp = dir.join(format!("{}.tmp", store::USAGE_SPEED_FILE));
    fs::write(&tmp, render_speed_js(ts_ms, &view.runs, &view.sessions)?)
        .map_err(|e| format!("写入 {} 失败: {e}", tmp.display()))?;
    fs::rename(&tmp, &target)
        .map_err(|e| format!("替换 {} 失败: {e}", target.display()))?;
    *cache = Some(payload);
    Ok(true)
}

/// 渲染心跳小文件内容（几十字节）：注入版宠物壳每 2 秒经 script 时间戳
/// 重载读取 window.__ZBAR_USAGE_HB__ 喂给宠物核心（heartbeat 接口）。
fn render_heartbeat_js(ms: i64) -> String {
    format!("window.__ZBAR_USAGE_HB__ = {ms};\n")
}

/// 心跳小文件写出：无条件重写（仅注入版宠物开启时被 export_once 调用，
/// 调用方保证宠物关闭时不写并清理残留），先写 .tmp 再 rename 原子替换。
fn write_heartbeat_file(dir: &Path, now_ms: i64) -> Result<(), String> {
    let target = dir.join(store::USAGE_HB_FILE);
    let tmp = dir.join(format!("{}.tmp", store::USAGE_HB_FILE));
    fs::write(&tmp, render_heartbeat_js(now_ms))
        .map_err(|e| format!("写入 {} 失败: {e}", tmp.display()))?;
    fs::rename(&tmp, &target).map_err(|e| format!("替换 {} 失败: {e}", target.display()))
}

/// 变化检测 + 原子写（大文件，恢复"内容不变跳写"策略）：turns 与 runs
/// 的序列化字节（含 pu/ta/fe 的字符串形态——用户发消息、工具开始/结束、
/// 失败轮落库本身就是数据变化，三者出现/消失/变化都触发写盘刷新 ts）
/// 拼接后与上轮相同则跳过写盘（la 是 turns/runs/pu/ta 的纯推导值，天然
/// 随内容参与变化语义——活动时刻变化本身就是数据变化；ts 字段不参与
/// 比较——若参与则每轮 ts 都不同，跳写失效，ZCode 渲染层每 2 秒白重载
/// 一次文件）；需要写出时先写 .tmp 再 rename，Electron 侧不会读到半截
/// 文件。返回是否实际写盘。
fn write_if_changed(
    dir: &Path,
    cache: &mut Option<String>,
    la_ms: i64,
    pending_user: Option<i64>,
    active_tool: Option<i64>,
    failure_event: Option<i64>,
    turns_json: &str,
    runs_json: &str,
    sess_json: &str,
    ts_ms: i64,
) -> Result<bool, String> {
    let mut payload =
        String::with_capacity(turns_json.len() + runs_json.len() + sess_json.len() + 16);
    payload.push_str(turns_json);
    payload.push('\u{1}'); /* 不可见分隔符：防多段拼接的边界歧义 */
    payload.push_str(runs_json);
    payload.push('\u{1}');
    payload.push_str(sess_json);
    payload.push('\u{1}');
    /* 附加信号形态并入对比键：None（'-'）与任一时刻值互不相同 */
    for sig in [pending_user, active_tool, failure_event] {
        match sig {
            Some(t) => payload.push_str(&t.to_string()),
            None => payload.push('-'),
        }
        payload.push('\u{1}');
    }
    if cache.as_deref() == Some(payload.as_str()) {
        return Ok(false);
    }
    let target = dir.join(store::USAGE_DATA_FILE);
    let tmp = dir.join(format!("{}.tmp", store::USAGE_DATA_FILE));
    fs::write(
        &tmp,
        render_usage_js(
            ts_ms,
            la_ms,
            pending_user,
            active_tool,
            failure_event,
            turns_json,
            runs_json,
            sess_json,
        ),
    )
    .map_err(|e| format!("写入 {} 失败: {e}", tmp.display()))?;
    fs::rename(&tmp, &target).map_err(|e| format!("替换 {} 失败: {e}", target.display()))?;
    *cache = Some(payload);
    Ok(true)
}

// ============================================================
// 单元测试（内存/临时 sqlite 构造，不依赖真实 ~/.zcode）
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::token_speed::SpeedQuality;

    /// 构造一轮主会话轮（其余字段取典型值，测试按需覆写）
    fn turn(id: &str, sess: &str, start: i64, end: Option<i64>) -> UsageTurn {
        UsageTurn {
            turn_id: id.to_string(),
            user_message_id: None,
            session_id: sess.to_string(),
            status: "completed".to_string(),
            start,
            end,
            input_tokens: 100,
            output_tokens: 200,
            cache_read: 50,
            cache_write: 10,
            reasoning: 5,
            requests: 2,
            retries: 0,
            tool_calls: 1,
            dur: Some(4000),
            ttft: Some(900),
            sub: None,
            subagent: None,
            models: String::new(),
            speed: None,
        }
    }

    /// 构造一条子代理轮原始行（其余字段取典型值，测试按需覆写）
    fn sub_turn(
        id: &str,
        sess: &str,
        parent: &str,
        start: i64,
        end: Option<i64>,
    ) -> SubTurnRow {
        SubTurnRow {
            turn_id: id.to_string(),
            session_id: sess.to_string(),
            parent_session_id: Some(parent.to_string()),
            status: "completed".to_string(),
            started_at: start,
            completed_at: end,
            user_message_id: Some(format!("msg_{id}")),
            dur: Some(350),
            ttft: Some(100),
            input_tokens: 30,
            output_tokens: 40,
            cache_read: 20,
            cache_write: 0,
            reasoning: 0,
            requests: 1,
            retries: 0,
            tool_calls: 0,
        }
    }

    #[test]
    fn 序列化_键名契约与渲染形态() {
        let mut t = turn("turn_a", "sess_1", 1000, Some(5000));
        t.status = "cancelled".to_string();
        t.user_message_id = Some("msg_u1".to_string());
        t.sub = Some(SubAgg {
            n: 2,
            requests: 5,
            input_tokens: 60,
            output_tokens: 80,
            cache_read: 40,
            cache_write: 0,
            reasoning: 0,
        });
        t.models = "GLM-5.3".to_string();
        let turns = vec![t];
        let json = serde_json::to_string(&turns).unwrap();
        // 短键名契约一字不差（usage.js 按名消费）
        for key in [
            "\"turn\":\"turn_a\"",
            "\"umid\":\"msg_u1\"",
            "\"sess\":\"sess_1\"",
            "\"status\":\"cancelled\"",
            "\"start\":1000",
            "\"end\":5000",
            "\"in\":100",
            "\"out\":200",
            "\"cr\":50",
            "\"cw\":10",
            "\"rt\":5",
            "\"req\":2",
            "\"retry\":0",
            "\"tool\":1",
            "\"dur\":4000",
            "\"ttft\":900",
            "\"models\":\"GLM-5.3\"",
        ] {
            assert!(json.contains(key), "缺少契约键 {key}：{json}");
        }
        // 子聚合对象
        assert!(
            json.contains("\"sub\":{\"n\":2,\"req\":5,\"in\":60,\"out\":80,\"cr\":40,\"cw\":0,\"rt\":0}"),
            "sub 聚合形态不符：{json}"
        );
        // V20：主会话行不携带 subagent 键（skip 序列化，向后兼容旧前端）
        assert!(!json.contains("\"subagent\""), "主会话行不应有 subagent 键：{json}");
        // V20：子代理自身视图行的 subagent:1 短键形态
        let mut sv = turn("turn_sv", "sess_subagent_agent_1", 1000, Some(5000));
        sv.user_message_id = Some("msg_child".to_string());
        sv.subagent = Some(1);
        let sv_json = serde_json::to_string(&vec![sv]).unwrap();
        assert!(sv_json.contains("\"subagent\":1"), "{sv_json}");
        assert!(
            !sv_json.contains("\"sub\":{"),
            "自身视图行不携带 sub 并入聚合：{sv_json}"
        );
        // 完整文件形态：v/ts/la/pu/ta/fe/turns/runs/sess 字段 + 分号结尾
        //（v2 格式不变，runs 为 V6 起追加的进行中轮字段、la 为 V2 起追加的
        // 最后活动时刻字段、pu 为 V5 起追加的待处理用户消息字段、ta/fe
        // 为 V6 起追加的活跃工具/失败轮事件字段、sess 为会话级统计附加
        // 字段（model_usage 全量合计）；旧消费端按未知字段忽略；
        // runs/sess 为空时也输出；心跳已拆独立小文件 usage-data-hb.js，
        // 大文件不再含 hb 字段）
        let file = render_usage_js(
            12345,
            12000,
            Some(11000),
            Some(10500),
            None,
            &json,
            "[]",
            "[]",
        );
        assert!(
            file.starts_with(
                "window.__ZBAR_USAGE__ = {\"v\":2,\"ts\":12345,\"la\":12000,\"pu\":11000,\"ta\":10500,\"fe\":null,\"turns\":"
            ),
            "{file}"
        );
        // pu/ta/fe 缺失两态之一：None → null（旧消费端与宠物核心按缺失兼容）
        let file_null = render_usage_js(12345, 12000, None, None, Some(9000), &json, "[]", "[]");
        assert!(file_null.contains(",\"pu\":null,"), "{file_null}");
        assert!(file_null.contains(",\"ta\":null,"), "{file_null}");
        assert!(file_null.contains(",\"fe\":9000,"), "{file_null}");
        assert!(!file.contains("\"hb\":"), "心跳不应在大文件里：{file}");
        assert!(file.contains(",\"runs\":[]"), "{file}");
        // sess 附加数组（V2 格式不变的追加字段，空数组也输出）
        assert!(file.contains(",\"sess\":[]};\n"), "{file}");
        assert!(file.ends_with("};\n"));
    }

    #[test]
    fn 速度小文件_只含窄快照且与大历史文件独立() {
        let dir = std::env::temp_dir().join(format!(
            "zbar-usage-speed-write-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        let snapshot = SpeedSnapshot {
            value: 125.0,
            quality: crate::token_speed::SpeedQuality::Generation,
            completed_at: 9_000,
            request_id: None,
        };
        let mut current_turn = turn("turn_speed", "sess_speed", 1_000, Some(9_000));
        current_turn.speed = Some(snapshot.clone());
        let turns = vec![current_turn];
        let sessions = vec![UsageSessionStat {
            session_id: "sess_speed".to_string(),
            total: 1_200,
            plain_in: 100,
            out_tokens: 1_000,
            cache_read: 100,
            requests: 1,
            speed: Some(snapshot),
            speed_state: SpeedState::Recent,
            member_session_ids: vec!["sess_speed".to_string()],
            round_started_at: None,
            generating: false,
        }];
        let view = SpeedView::from_rows(&[], &sessions, Some("sess_speed"));
        let mut speed_cache = None;
        assert!(write_speed_view(&mut speed_cache, &dir, &view, 10_000).unwrap());
        let speed_path = dir.join(store::USAGE_SPEED_FILE);
        let first = fs::read_to_string(&speed_path).unwrap();
        assert!(first.starts_with("window.__ZBAR_USAGE_SPEED__ = "), "{first}");
        assert!(first.contains("\"v\":1"), "{first}");
        assert!(first.contains("\"quality\":\"generation\""), "{first}");
        assert!(first.contains("\"speedState\":\"recent\""), "{first}");
        assert!(first.contains("\"turns\":[]"), "A3 旁路不应携带历史轮: {first}");
        for token_key in ["\"in\":", "\"out\":", "\"cr\":", "\"tt\":", "\"down\":"] {
            assert!(
                !first.contains(token_key),
                "速度小文件不应携带历史累计字段 {token_key}: {first}"
            );
        }

        // 只有 ts 变化时不重写；payload 未变，文件字节也保持不变。
        assert!(!write_speed_view(&mut speed_cache, &dir, &view, 11_000).unwrap());
        assert_eq!(fs::read_to_string(&speed_path).unwrap(), first);

        // 速度发生变化才重写；临时文件不会残留。
        let mut changed = view.clone();
        changed.sessions[0].speed.as_mut().unwrap().value = 130.0;
        assert!(write_speed_view(&mut speed_cache, &dir, &changed, 12_000).unwrap());
        let changed_text = fs::read_to_string(&speed_path).unwrap();
        assert!(changed_text.contains("130.0"), "{changed_text}");
        assert!(!dir.join(format!("{}.tmp", store::USAGE_SPEED_FILE)).exists());

        // A3 分工：完成轮的稳定速度与空闲会话的速度/状态随大文件发布；
        // 进行中轮（runs）的速度不在大文件（旁路 1 秒覆盖）。
        let mut history_cache = None;
        let mut run_row = UsageRun {
            turn_id: "turn_running".to_string(),
            user_message_id: Some("msg_running".to_string()),
            session_id: "sess_speed".to_string(),
            parent_session_id: None,
            input_tokens: 10,
            output_tokens: 20,
            cache_read: 0,
            cache_write: 0,
            reasoning: 0,
            requests: 1,
            start: 9_500,
            merged: None,
            sub: None,
            speed: Some(SpeedSnapshot {
                value: 99.0,
                quality: crate::token_speed::SpeedQuality::Generation,
                completed_at: 9_800,
                request_id: None,
            }),
        };
        flush_export(
            &dir,
            &mut history_cache,
            false,
            &turns,
            &[run_row.clone()],
            &sessions,
            None,
            None,
            None,
            12_000,
        )
        .unwrap();
        let history = fs::read_to_string(dir.join(store::USAGE_DATA_FILE)).unwrap();
        assert!(
            history.contains("\"speed\":{\"value\":125.0"),
            "完成轮的稳定速度应随大文件发布: {history}"
        );
        assert!(
            history.contains("\"speedState\":\"recent\""),
            "空闲会话的速度状态应随大文件发布: {history}"
        );
        let runs_start = history.find("\"runs\":").unwrap();
        let runs_end = history[runs_start..].find(",\"sess\":").unwrap() + runs_start;
        assert!(
            !history[runs_start..runs_end].contains("\"speed\""),
            "进行中轮的速度不应进入大文件: {}",
            &history[runs_start..runs_end]
        );
        run_row.speed = None;
        let _ = run_row;

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn 速度会话刷新_沿树取最新且待处理轮不恢复旧值() {
        let mut catalog = SpeedCatalog::default();
        let root_request = LatestUsageRequest {
            order: 1_200,
            timing: RequestTiming {
                output_tokens: 100,
                started_at: Some(1_000),
                first_token_at: Some(1_100),
                completed_at: Some(1_200),
                ..RequestTiming::default()
            },
        };
        let child_request = LatestUsageRequest {
            order: 2_300,
            timing: RequestTiming {
                output_tokens: 300,
                started_at: Some(2_000),
                first_token_at: Some(2_100),
                completed_at: Some(2_300),
                ..RequestTiming::default()
            },
        };
        catalog
            .by_session
            .insert("sess_root".to_string(), root_request);
        catalog
            .by_session
            .insert("sess_child".to_string(), child_request);

        let mut view = SpeedView {
            sessions: vec![SpeedSessionEntry {
                session_id: "sess_root".to_string(),
                speed: None,
                speed_state: SpeedState::Unavailable,
                member_session_ids: vec!["sess_root".to_string(), "sess_child".to_string()],
                round_started_at: None,
                generating: false,
            }],
            ..SpeedView::default()
        };
        view.refresh(&catalog, 3_000);
        assert_eq!(view.sessions[0].speed.as_ref().unwrap().value, 1_500.0);

        // A new user message starts a round before its first request exists.
        // The same catalog must not restore either historical member speed.
        view.sessions[0].speed = Some(SpeedSnapshot {
            value: 1_500.0,
            quality: SpeedQuality::Generation,
            completed_at: 2_300,
            request_id: None,
        });
        view.sessions[0].speed_state = SpeedState::Measuring;
        view.sessions[0].generating = true;
        view.sessions[0].round_started_at = Some(2_500);
        view.refresh(&catalog, 3_000);
        assert_eq!(view.sessions[0].speed, None);
        assert_eq!(view.sessions[0].speed_state, SpeedState::Measuring);
    }

    #[test]
    fn 速度会话刷新_生成中速度暂时为空时回落measuring而不是unavailable() {
        // A2 场景一：同轮第一请求完成（Recent）→ 第二请求进行中（最新行
        // 无效，速度清空）→ 状态必须是 Measuring（仍在生成），绝不能因
        // "没有速度"反推成 Unavailable；第二请求完成后恢复 Recent；轮结
        // 束（generating=false）且无有效速度时才是 Unavailable。
        let mut catalog = SpeedCatalog::default();
        let first_request = LatestUsageRequest {
            order: 1_200,
            timing: RequestTiming {
                output_tokens: 100,
                started_at: Some(1_000),
                first_token_at: Some(1_100),
                completed_at: Some(1_200),
                ..RequestTiming::default()
            },
        };
        catalog
            .by_session
            .insert("sess_a2".to_string(), first_request);
        let mut view = SpeedView {
            sessions: vec![SpeedSessionEntry {
                session_id: "sess_a2".to_string(),
                speed: None,
                speed_state: SpeedState::Unavailable,
                member_session_ids: vec!["sess_a2".to_string()],
                round_started_at: None,
                generating: true,
            }],
            ..SpeedView::default()
        };
        view.refresh(&catalog, 2_000);
        // 正在生成 + 本轮已完成请求有最近速度：generating=true 且 Recent
        assert_eq!(view.sessions[0].speed_state, SpeedState::Recent);
        assert!(view.sessions[0].generating);

        // 第二请求进行中：目录被最新的进行中行覆盖（无效候选），速度清空
        let mut catalog_second = SpeedCatalog::default();
        catalog_second.by_session.insert(
            "sess_a2".to_string(),
            LatestUsageRequest {
                order: 3_000,
                timing: RequestTiming {
                    // 非完成状态的最新行：output 置 0，速度无效
                    output_tokens: 0,
                    started_at: Some(2_500),
                    first_token_at: None,
                    completed_at: None,
                    ..RequestTiming::default()
                },
            },
        );
        view.refresh(&catalog_second, 3_000);
        assert_eq!(view.sessions[0].speed, None, "进行中请求不得沿用旧速度");
        assert_eq!(
            view.sessions[0].speed_state,
            SpeedState::Measuring,
            "生成中且速度暂时为空应回落 measuring，不得变成 unavailable"
        );

        // 第二请求完成：恢复 Recent
        let mut catalog_done = SpeedCatalog::default();
        catalog_done.by_session.insert(
            "sess_a2".to_string(),
            LatestUsageRequest {
                order: 5_000,
                timing: RequestTiming {
                    output_tokens: 200,
                    started_at: Some(2_500),
                    first_token_at: Some(2_600),
                    completed_at: Some(5_000),
                    ..RequestTiming::default()
                },
            },
        );
        view.refresh(&catalog_done, 5_500);
        assert_eq!(view.sessions[0].speed_state, SpeedState::Recent);

        // 轮结束：无新一轮待处理消息 → generating=false；目录中最新行又
        // 变为无效（例如失败行）时状态为 Unavailable（不是 Measuring）
        view.sessions[0].generating = false;
        view.refresh(&catalog_second, 6_000);
        assert_eq!(view.sessions[0].speed, None);
        assert_eq!(view.sessions[0].speed_state, SpeedState::Unavailable);

        // 场景二：新轮没有已确认速度（round_started_at 屏蔽上一轮请求）
        view.sessions[0].generating = true;
        view.sessions[0].round_started_at = Some(6_500);
        view.refresh(&catalog_done, 7_000);
        assert_eq!(view.sessions[0].speed, None, "新轮不得回填上一轮速度");
        assert_eq!(view.sessions[0].speed_state, SpeedState::Measuring);
    }

    #[test]
    fn 速度旁路视图_只保留活跃会话与进行中轮() {
        // A3：3000 历史轮 + 若干空闲会话 + 1 个活跃会话（含进行中轮）。
        // 旁路只携带活跃会话条目与 runs；turns 恒为空。
        let mut sessions = Vec::new();
        for i in 0..40 {
            sessions.push(UsageSessionStat {
                session_id: format!("sess_idle_{i}"),
                total: 1_000,
                plain_in: 100,
                out_tokens: 800,
                cache_read: 100,
                requests: 9,
                speed: Some(SpeedSnapshot {
                    value: 120.0,
                    quality: SpeedQuality::Generation,
                    completed_at: 1_000,
                    request_id: None,
                }),
                speed_state: SpeedState::Recent,
                member_session_ids: vec![format!("sess_idle_{i}")],
                round_started_at: None,
                generating: false,
            });
        }
        sessions.push(UsageSessionStat {
            session_id: "sess_active".to_string(),
            total: 2_000,
            plain_in: 200,
            out_tokens: 1_600,
            cache_read: 200,
            requests: 4,
            speed: None,
            speed_state: SpeedState::Measuring,
            member_session_ids: vec![
                "sess_active".to_string(),
                "sess_subagent_agent_1".to_string(),
            ],
            round_started_at: Some(5_000),
            generating: true,
        });
        let runs = vec![UsageRun {
            turn_id: "turn_live".to_string(),
            user_message_id: Some("msg_live".to_string()),
            session_id: "sess_subagent_agent_1".to_string(),
            parent_session_id: Some("sess_active".to_string()),
            input_tokens: 10,
            output_tokens: 20,
            cache_read: 0,
            cache_write: 0,
            reasoning: 0,
            requests: 1,
            start: 5_100,
            merged: None,
            sub: None,
            speed: None,
        }];
        let view = SpeedView::from_rows(&runs, &sessions, Some("sess_active"));

        let dir = std::env::temp_dir().join(format!(
            "zbar-usage-speed-slim-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let mut speed_cache = None;
        assert!(write_speed_view(&mut speed_cache, &dir, &view, 7_000).unwrap());
        let speed_path = dir.join(store::USAGE_SPEED_FILE);
        let text = fs::read_to_string(&speed_path).unwrap();
        let bytes_idle = fs::metadata(&speed_path).unwrap().len();
        eprintln!(
            "[speed-bypass-bytes] 单活跃会话+1进行中轮: {bytes_idle} B（3000 历史轮/40 空闲会话不进旁路）"
        );
        assert!(text.contains("\"s\":\"sess_active\""), "{text}");
        assert!(
            !text.contains("sess_idle_"),
            "旁路不应携带空闲会话: {text}"
        );
        assert!(
            !text.contains("sess_idle_39") && text.matches("\"s\":").count() == 1,
            "旁路应只有活跃会话条目: {text}"
        );
        assert!(text.contains("\"turns\":[]"), "{text}");
        // 空闲 + 单活跃会话 + 3000 历史轮场景下的旁路字节数（远小于大文件）
        assert!(
            bytes_idle < 1_024,
            "空闲+单活跃会话的旁路应远小于 1KiB: {bytes_idle}"
        );

        // 3000 历史轮场景：大文件 turns 行数不影响旁路字节数（旁路不含
        // turns）；一笔速度变化的写入量 = 整个旁路文件的字节数（原子替换）
        // —— 空闲场景（无活跃会话/轮）旁路为空集，不写盘（session_ids 为空
        // 时 export_speed_once 直接返回）。
        let mut changed = view.clone();
        changed.sessions[0].speed = Some(SpeedSnapshot {
            value: 88.8,
            quality: SpeedQuality::Generation,
            completed_at: 6_800,
            request_id: None,
        });
        changed.runs[0].speed = Some(SpeedSnapshot {
            value: 77.7,
            quality: SpeedQuality::Generation,
            completed_at: 6_900,
            request_id: None,
        });
        assert!(write_speed_view(&mut speed_cache, &dir, &changed, 7_100).unwrap());
        let changed_bytes = fs::metadata(&speed_path).unwrap().len();
        eprintln!("[speed-bypass-bytes] 一笔速度变化的写入量: {changed_bytes} B");
        assert!(
            changed_bytes < 1_024,
            "一笔速度变化的旁路写入量应远小于 1KiB: {changed_bytes}"
        );
        assert!(changed_bytes > bytes_idle, "速度变化应真实写盘");
        let changed_text = fs::read_to_string(&speed_path).unwrap();
        assert!(changed_text.contains("88.8"), "{changed_text}");
        assert!(changed_text.contains("77.7"), "{changed_text}");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn runs序列化_键名契约与短键形态() {
        let r = UsageRun {
            turn_id: "turn_r".to_string(),
            user_message_id: Some("msg_u1".to_string()),
            session_id: "sess_1".to_string(),
            parent_session_id: None,
            input_tokens: 700,
            output_tokens: 80,
            cache_read: 600,
            cache_write: 20,
            reasoning: 9,
            requests: 3,
            start: 42,
            merged: None,
            sub: None,
            speed: None,
        };
        let json = serde_json::to_string(&vec![r]).unwrap();
        // 短键名契约一字不差（usage.js 按名消费）；无 turn/status 等整轮字段；
        // m/sub 为 None 时不输出（V9 附加字段，主会话行无子代理并入时保持
        // 旧形态，向后兼容）
        assert_eq!(
            json,
            "[{\"umid\":\"msg_u1\",\"sess\":\"sess_1\",\"psess\":null,\
              \"in\":700,\"out\":80,\"cr\":600,\"cw\":20,\"rt\":9,\
              \"req\":3,\"start\":42}]",
            "runs 行序列化形态不符：{json}"
        );
        // umid null + 子代理 psess 形态
        let sub_run = UsageRun {
            turn_id: "turn_sub".to_string(),
            user_message_id: None,
            session_id: "sess_subagent_agent_1".to_string(),
            parent_session_id: Some("sess_main".to_string()),
            input_tokens: 0,
            output_tokens: 0,
            cache_read: 0,
            cache_write: 0,
            reasoning: 0,
            requests: 0,
            start: 1,
            merged: None,
            sub: None,
            speed: None,
        };
        let json = serde_json::to_string(&vec![sub_run]).unwrap();
        assert!(json.contains("\"umid\":null"), "{json}");
        assert!(json.contains("\"psess\":\"sess_main\""), "{json}");
        assert!(!json.contains("\"m\":"), "未并入的子代理行不打 m 标记：{json}");
        // V9：m:1 标记 + sub 聚合的短键形态（结构与 turns 行 sub 一致）
        let merged_run = UsageRun {
            turn_id: "turn_merged".to_string(),
            user_message_id: Some("msg_main".to_string()),
            session_id: "sess_main".to_string(),
            parent_session_id: None,
            input_tokens: 100,
            output_tokens: 200,
            cache_read: 50,
            cache_write: 10,
            reasoning: 5,
            requests: 2,
            start: 7,
            merged: None,
            sub: Some(SubAgg {
                n: 3,
                requests: 9,
                input_tokens: 90,
                output_tokens: 120,
                cache_read: 60,
                cache_write: 0,
                reasoning: 1,
            }),
            speed: None,
        };
        let json = serde_json::to_string(&vec![merged_run]).unwrap();
        assert!(
            json.contains("\"sub\":{\"n\":3,\"req\":9,\"in\":90,\"out\":120,\"cr\":60,\"cw\":0,\"rt\":1}"),
            "sub 聚合形态应与 turns 行 sub 一致：{json}"
        );
        assert!(!json.contains("\"m\":"), "主会话行不打 m 标记：{json}");
        // 子代理行 m:1
        let marked = UsageRun {
            session_id: "sess_subagent_agent_1".to_string(),
            parent_session_id: Some("sess_main".to_string()),
            merged: Some(1),
            ..sub_run_template()
        };
        let json = serde_json::to_string(&vec![marked]).unwrap();
        assert!(json.contains(",\"m\":1}"), "已并入主轮行 sub 的子代理行应带 m:1：{json}");
        assert!(!json.contains("\"sub\":"), "子代理行不携带 sub：{json}");
    }

    /// 序列化测试用子代理行模板（测试按需覆写）
    fn sub_run_template() -> UsageRun {
        UsageRun {
            turn_id: "turn_template".to_string(),
            user_message_id: None,
            session_id: "sess_subagent_agent_1".to_string(),
            parent_session_id: Some("sess_main".to_string()),
            input_tokens: 0,
            output_tokens: 0,
            cache_read: 0,
            cache_write: 0,
            reasoning: 0,
            requests: 0,
            start: 1,
            merged: None,
            sub: None,
            speed: None,
        }
    }

    #[test]
    fn 大文件写盘_内容不变跳写_变化才重写且无临时残留() {
        let dir = std::env::temp_dir().join(format!(
            "zbar-usage-feed-write-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let target = dir.join(store::USAGE_DATA_FILE);

        let mut cache: Option<String> = None;
        // 首次：写盘（la 为最后活动时刻，随内容透出）
        assert!(
            write_if_changed(&dir, &mut cache, 900, None, None, None, "[{\"turn\":\"a\"}]", "[]", "[]", 1000)
                .unwrap()
        );
        let first = fs::read_to_string(&target).unwrap();
        assert!(first.contains("\"ts\":1000"), "{first}");
        assert!(first.contains("\"la\":900"), "{first}");
        assert!(first.contains("\"pu\":null"), "{first}");
        assert!(first.contains("\"ta\":null"), "{first}");
        assert!(first.contains("\"fe\":null"), "{first}");
        assert!(first.contains("\"runs\":[]"), "{first}");
        assert!(first.contains("\"sess\":[]"), "{first}");
        // 内容无变化（仅 ts/la 参数不同）→ 跳写，文件保持旧值（写放大
        // 修复：大文件恢复跳写策略，心跳由独立小文件承担）
        assert!(
            !write_if_changed(&dir, &mut cache, 900, None, None, None, "[{\"turn\":\"a\"}]", "[]", "[]", 2000)
                .unwrap()
        );
        assert_eq!(
            fs::read_to_string(&target).unwrap(),
            first,
            "内容未变时不应重写大文件"
        );
        // turns 内容变化 → 重写为新 ts
        assert!(
            write_if_changed(&dir, &mut cache, 900, None, None, None, "[{\"turn\":\"b\"}]", "[]", "[]", 3000)
                .unwrap()
        );
        let third = fs::read_to_string(&target).unwrap();
        assert!(third.contains("\"ts\":3000"), "{third}");
        assert!(third.contains("\"turn\":\"b\""));
        // runs 内容变化（turns 不变）同样触发重写——进行中轮实时聚合
        // 每 2 秒跳动天然走到这里
        assert!(
            write_if_changed(
                &dir,
                &mut cache,
                900,
                None,
                None,
                None,
                "[{\"turn\":\"b\"}]",
                "[{\"sess\":\"s1\"}]",
                "[]",
                4000
            )
            .unwrap()
        );
        let fourth = fs::read_to_string(&target).unwrap();
        assert!(fourth.contains("\"ts\":4000"), "{fourth}");
        assert!(fourth.contains("\"runs\":[{\"sess\":\"s1\"}]"), "{fourth}");
        // sess 内容变化（turns/runs 均不变）同样触发重写——会话级统计
        //（全量合计随请求落库推进）本身就是数据变化
        assert!(
            write_if_changed(
                &dir,
                &mut cache,
                900,
                None,
                None,
                None,
                "[{\"turn\":\"b\"}]",
                "[{\"sess\":\"s1\"}]",
                "[{\"s\":\"s1\",\"tt\":123}]",
                4500
            )
            .unwrap()
        );
        let fourth_half = fs::read_to_string(&target).unwrap();
        assert!(fourth_half.contains("\"ts\":4500"), "{fourth_half}");
        assert!(
            fourth_half.contains("\"sess\":[{\"s\":\"s1\",\"tt\":123}]"),
            "{fourth_half}"
        );
        // pu 变化（turns/runs/sess 均不变）同样触发重写——用户发消息本身就是
        // 数据变化（V5）：ts 刷新、pu 透出、la 取大（900 → 5000）
        assert!(
            write_if_changed(
                &dir,
                &mut cache,
                5000,
                Some(5000),
                None,
                None,
                "[{\"turn\":\"b\"}]",
                "[{\"sess\":\"s1\"}]",
                "[{\"s\":\"s1\",\"tt\":123}]",
                5000
            )
            .unwrap()
        );
        let fifth = fs::read_to_string(&target).unwrap();
        assert!(fifth.contains("\"ts\":5000"), "{fifth}");
        assert!(fifth.contains("\"pu\":5000"), "{fifth}");
        assert!(fifth.contains("\"la\":5000"), "pu 应参与 la 取大：{fifth}");
        // ta/fe 变化（turns/runs/pu 均不变）同样触发重写——工具开始
        // （V6）本身就是数据变化：ts 刷新、ta/fe 透出、ta 参与 la 取大
        assert!(
            write_if_changed(
                &dir,
                &mut cache,
                6000,
                Some(5000),
                Some(6000),
                None,
                "[{\"turn\":\"b\"}]",
                "[{\"sess\":\"s1\"}]",
                "[{\"s\":\"s1\",\"tt\":123}]",
                6000
            )
            .unwrap()
        );
        let sixth = fs::read_to_string(&target).unwrap();
        assert!(sixth.contains("\"ts\":6000"), "{sixth}");
        assert!(sixth.contains("\"ta\":6000"), "{sixth}");
        assert!(sixth.contains("\"la\":6000"), "ta 应参与 la 取大：{sixth}");
        assert!(
            write_if_changed(
                &dir,
                &mut cache,
                6000,
                Some(5000),
                Some(6000),
                Some(5500),
                "[{\"turn\":\"b\"}]",
                "[{\"sess\":\"s1\"}]",
                "[{\"s\":\"s1\",\"tt\":123}]",
                7000
            )
            .unwrap()
        );
        let seventh = fs::read_to_string(&target).unwrap();
        assert!(seventh.contains("\"fe\":5500"), "{seventh}");
        // 原子写不留 .tmp 残留
        assert!(!dir.join(format!("{}.tmp", store::USAGE_DATA_FILE)).exists());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn 心跳小文件_内容形态与周期重写() {
        let dir = std::env::temp_dir().join(format!(
            "zbar-usage-feed-hb-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let target = dir.join(store::USAGE_HB_FILE);

        // 内容形态：几十字节全局赋值（注入版宠物壳消费 window.__ZBAR_USAGE_HB__）
        write_heartbeat_file(&dir, 1000).unwrap();
        assert_eq!(
            fs::read_to_string(&target).unwrap(),
            "window.__ZBAR_USAGE_HB__ = 1000;\n"
        );
        // 每周期无条件重写（宠物开启时调用方每 2 秒调一次）
        write_heartbeat_file(&dir, 3000).unwrap();
        assert_eq!(
            fs::read_to_string(&target).unwrap(),
            "window.__ZBAR_USAGE_HB__ = 3000;\n"
        );
        // 不留 .tmp 残留
        assert!(!dir.join(format!("{}.tmp", store::USAGE_HB_FILE)).exists());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn 导出落盘_宠物关不写心跳且大文件跳写_宠物开心跳每周期刷新() {
        let dir = std::env::temp_dir().join(format!(
            "zbar-usage-feed-flush-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let big = dir.join(store::USAGE_DATA_FILE);
        let hb = dir.join(store::USAGE_HB_FILE);

        let turns = vec![turn("turn_a", "sess_1", 1000, Some(5000))];
        let mut cache: Option<String> = None;

        // ---- 宠物关：大文件按变化写、心跳不写、残留被清理 ----
        // 预置心跳残留（模拟此前宠物开启过）
        fs::write(&hb, b"window.__ZBAR_USAGE_HB__ = 1;\n").unwrap();
        let sess = vec![UsageSessionStat {
            session_id: "sess_1".to_string(),
            total: 1110,
            plain_in: 100,
            out_tokens: 200,
            cache_read: 50,
            requests: 2,
            speed: None,
            speed_state: SpeedState::Unavailable,
            member_session_ids: vec!["sess_1".to_string()],
            round_started_at: None,
            generating: false,
        }];
        flush_export(&dir, &mut cache, false, &turns, &[], &sess, None, None, None, 1000).unwrap();
        assert!(big.exists(), "首次导出应写大文件");
        let first = fs::read_to_string(&big).unwrap();
        assert!(first.contains("\"ts\":1000"), "{first}");
        assert!(first.contains("\"pu\":null"), "{first}");
        assert!(!hb.exists(), "宠物关闭应清理心跳残留");
        // 内容不变 → 大文件跳写（mtime 不变以内容一致性表达），心跳仍不写
        flush_export(&dir, &mut cache, false, &turns, &[], &sess, None, None, None, 2000).unwrap();
        assert_eq!(fs::read_to_string(&big).unwrap(), first, "内容未变不应重写大文件");
        assert!(!hb.exists(), "宠物关闭周期不应写心跳文件");

        // ---- 宠物开：心跳每周期无条件刷新、大文件仍按变化写 ----
        let mut cache_on: Option<String> = None;
        flush_export(&dir, &mut cache_on, true, &turns, &[], &sess, Some(4500), None, None, 3000).unwrap();
        let big_on = fs::read_to_string(&big).unwrap();
        assert!(big_on.contains("\"ts\":3000"), "首次写盘 ts 应为当前周期");
        assert!(big_on.contains("\"la\":5000"), "la 应随内容透出：{big_on}");
        assert!(
            big_on.contains("\"pu\":4500"),
            "pu 应随内容透出：{big_on}"
        );
        assert_eq!(
            fs::read_to_string(&hb).unwrap(),
            "window.__ZBAR_USAGE_HB__ = 3000;\n"
        );
        // 下一周期内容不变：大文件跳写（保持旧 ts），心跳刷新为当前周期
        flush_export(&dir, &mut cache_on, true, &turns, &[], &sess, Some(4500), None, None, 5000).unwrap();
        assert_eq!(
            fs::read_to_string(&big).unwrap(),
            big_on,
            "内容未变大文件不应重写（心跳已独立承担存活信号）"
        );
        assert_eq!(
            fs::read_to_string(&hb).unwrap(),
            "window.__ZBAR_USAGE_HB__ = 5000;\n",
            "宠物开启时心跳应每周期刷新"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn 最后活动时刻_turns_end与runs_start取大_滑出窗口不误醒() {
        // 纯函数：完成轮取 end（精确值）、进行中轮取 start（首请求近似，
        // 见模块头口径），全体取 max；无任何活动为 0
        let t1 = turn("turn_1", "sess_1", 1000, Some(5000));
        let t2 = turn("turn_2", "sess_1", 2000, Some(9000));
        let t_noend = turn("turn_3", "sess_1", 3000, None);
        let run = UsageRun {
            turn_id: "turn_activity".to_string(),
            user_message_id: None,
            session_id: "sess_1".to_string(),
            parent_session_id: None,
            input_tokens: 0,
            output_tokens: 0,
            cache_read: 0,
            cache_write: 0,
            reasoning: 0,
            requests: 1,
            start: 12000,
            merged: None,
            sub: None,
            speed: None,
        };
        assert_eq!(last_activity_ms(&[], &[], None, None), 0, "无活动应为 0");
        assert_eq!(
            last_activity_ms(&[t1.clone(), t2.clone(), t_noend.clone()], &[], None, None),
            9000,
            "取完成轮 end 的最大值（缺 end 的行不参与）"
        );
        assert_eq!(
            last_activity_ms(&[t1], &[run], None, None),
            12000,
            "进行中轮 start 参与取大"
        );
        // pu 参与取大（V5）：用户发消息本身就是活动，且能压过完成轮 end
        assert_eq!(last_activity_ms(&[t2.clone()], &[], Some(9500), None), 9500);
        assert_eq!(
            last_activity_ms(&[t2.clone()], &[], Some(8000), None),
            9000,
            "更早的 pu 不拉低 la（取大语义）"
        );
        // ta 参与取大（V6）：工具开始也是活动，能压过 pu 与完成轮 end；
        // 更早的 ta 不拉低 la
        assert_eq!(
            last_activity_ms(&[t2.clone()], &[], Some(9500), Some(9800)),
            9800,
            "ta 应参与 la 取大（V6）"
        );
        assert_eq!(
            last_activity_ms(&[t2.clone()], &[], Some(9500), Some(8000)),
            9500,
            "更早的 ta 不拉低 la（取大语义）"
        );

        // 滑出窗口场景：旧轮退出 turns 序列（内容变化 → ts 刷新），la
        // 只会变小（取 max 的旧值被压掉）而不会被"无活动"刷新为 now——
        // 闲置判定按 la 计算，宠物不会因窗口滑动误弹回闲置
        let before = last_activity_ms(&[t2.clone(), t_noend.clone()], &[], None, None);
        let after = last_activity_ms(&[t_noend], &[], None, None); // t2 滑出 1 小时窗口
        assert_eq!(before, 9000);
        assert!(after < before, "轮滑出后 la 只会变小：{after}");
        assert_eq!(after, 0, "剩余行无 end 时 la 归 0（更快入睡）");
        let _ = t2;
    }

    #[test]
    fn 子代理并入_时间窗口聚合_未命中分流游离子轮与越界丢弃() {
        let mut turns = vec![
            turn("turn_main", "sess_main", 1000, Some(5000)),
            turn("turn_other", "sess_other", 1000, Some(9000)),
        ];
        let subs = vec![
            // 命中：父会话一致且时间被主轮覆盖
            sub_turn("turn_sub1", "sess_sub_a", "sess_main", 1100, Some(1500)),
            sub_turn("turn_sub2", "sess_sub_b", "sess_main", 2000, Some(3000)),
            // 未命中一：父会话不一致（属于别的会话）
            sub_turn("turn_sub3", "sess_sub_c", "sess_other", 1200, Some(1400)),
            // 未命中二：完成时刻缺失
            SubTurnRow {
                completed_at: None,
                ..sub_turn("turn_sub4", "sess_sub_d", "sess_main", 1100, Some(1500))
            },
            // 未命中三：时间不被任何主轮覆盖（早于主轮开始）→ 所属主轮
            // 尚未落库，作为游离子代理完成轮输出给 runs 侧聚合
            sub_turn("turn_sub5", "sess_sub_e", "sess_main", 500, Some(900)),
            // 未命中四：子轮开始于主轮窗口内但完成晚于主轮完成（越界）→
            // 所属主轮已落库（覆盖子轮开始时刻），维持丢弃防双计
            sub_turn("turn_sub6", "sess_sub_f", "sess_main", 2000, Some(6000)),
        ];
        let (pairs, orphans) = merge_subagent_turns(&mut turns, subs);

        // 并入明细：仅三条命中（turn_main ← sub1/sub2；turn_other ← sub3）
        assert_eq!(
            pairs,
            vec![
                ("turn_main".to_string(), "turn_sub1".to_string()),
                ("turn_main".to_string(), "turn_sub2".to_string()),
                ("turn_other".to_string(), "turn_sub3".to_string()),
            ]
        );

        let main = turns.iter().find(|t| t.turn_id == "turn_main").unwrap();
        // token 与次数累加（自身 100/200/50 + 两条子轮 30+30 / 40+40 / 20+20）
        assert_eq!(main.input_tokens, 160);
        assert_eq!(main.output_tokens, 280);
        assert_eq!(main.cache_read, 90);
        assert_eq!(main.requests, 4);
        // dur/ttft 保持主轮自身口径，不随并入变化
        assert_eq!(main.dur, Some(4000));
        assert_eq!(main.ttft, Some(900));
        // 子聚合明细（次数与 token 合计）
        let sub = main.sub.as_ref().expect("并入后应有子聚合");
        assert_eq!(sub.n, 2);
        assert_eq!(sub.requests, 2);
        assert_eq!(sub.input_tokens, 60);
        assert_eq!(sub.output_tokens, 80);
        assert_eq!(sub.cache_read, 40);

        let other = turns.iter().find(|t| t.turn_id == "turn_other").unwrap();
        assert_eq!(other.input_tokens, 130, "另一会话主轮应并入自己的子轮");
        assert_eq!(other.sub.as_ref().unwrap().n, 1);

        // 游离子代理完成轮：仅 sub5（所属主轮未落库）；sub4（无完成时刻）
        // 与 sub6（越界，所属主轮已落库）不输出
        assert_eq!(orphans.len(), 1, "游离子轮应仅含 sub5：{orphans:?}");
        assert_eq!(orphans[0].turn_id, "turn_sub5");
    }

    #[test]
    fn 模型清单_主轮并入子轮去重拼接() {
        let mut turns = vec![turn("turn_main", "sess_main", 1000, Some(5000))];
        let mut models = BTreeMap::new();
        models.insert("turn_main".to_string(), vec!["GLM-5.3".to_string()]);
        models.insert(
            "turn_sub1".to_string(),
            vec!["GLM-5.3".to_string(), "GLM-4.7".to_string()],
        );
        let pairs = vec![("turn_main".to_string(), "turn_sub1".to_string())];
        attach_models(&mut turns, &models, &pairs);
        assert_eq!(turns[0].models, "GLM-5.3,GLM-4.7", "应去重合并主轮与子轮模型");

        // 无模型记录的轮 → 空串（前端不显示模型）
        let mut turns = vec![turn("turn_x", "sess", 1, Some(2))];
        attach_models(&mut turns, &BTreeMap::new(), &[]);
        assert_eq!(turns[0].models, "");
    }

    #[test]
    fn build_models_map_去重保序() {
        let map = build_models_map(vec![
            ("t1".into(), "b-model".into()),
            ("t1".into(), "a-model".into()),
            ("t1".into(), "b-model".into()), // 重复
            ("t2".into(), "m".into()),
        ]);
        assert_eq!(map.get("t1").unwrap(), &vec!["b-model".to_string(), "a-model".to_string()]);
        assert_eq!(map.get("t2").unwrap(), &vec!["m".to_string()]);
    }

    /// 临时 sqlite 库（文件形， rusqlite 直连，测试结束清理）
    fn temp_db(name: &str) -> (Connection, std::path::PathBuf) {
        let path = std::env::temp_dir().join(format!(
            "zbar-usage-feed-db-{}-{name}.sqlite",
            std::process::id()
        ));
        let _ = fs::remove_file(&path);
        let conn = Connection::open(&path).unwrap();
        (conn, path)
    }

    #[test]
    fn 无turn_usage表时功能禁用_核心列缺失同样禁用() {
        // 场景一：只有 session 表，无 turn_usage 表 → Ok(None) 功能关闭
        let (conn, path) = temp_db("no-table");
        conn.execute_batch("CREATE TABLE session (id TEXT PRIMARY KEY, parent_id TEXT);")
            .unwrap();
        let out = collect_turns(&conn, 0).unwrap();
        assert!(out.is_none(), "无 turn_usage 表应返回 None 禁用功能");
        drop(conn);
        let _ = fs::remove_file(&path);

        // 场景二：turn_usage 表存在但缺核心列（极端老版本）→ Ok(None)
        let (conn, path) = temp_db("missing-core");
        conn.execute_batch("CREATE TABLE turn_usage (session_id TEXT, turn_id TEXT);")
            .unwrap();
        let out = collect_turns(&conn, 0).unwrap();
        assert!(out.is_none(), "缺核心列应返回 None 禁用功能");
        drop(conn);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn 有表时降级查询_缺列按0与null兜底() {
        // 老版本形态：turn_usage 只有 4 个核心列（无任何统计/耗时列），
        // 也无 session / model_usage 表 → 能出数据、数值全 0、耗时为 null
        let (conn, path) = temp_db("minimal");
        conn.execute_batch(
            "CREATE TABLE turn_usage (
                session_id TEXT, turn_id TEXT, status TEXT, started_at INTEGER);",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO turn_usage VALUES ('sess_1', 'turn_1', 'completed', 1000)",
            [],
        )
        .unwrap();
        let (out, orphans) = collect_turns(&conn, 0).unwrap().expect("核心列齐备应有输出");
        assert_eq!(out.len(), 1);
        let t = &out[0];
        assert_eq!(t.turn_id, "turn_1");
        assert_eq!(t.status, "completed");
        assert_eq!(t.input_tokens, 0, "缺列应按 0 降级");
        assert_eq!(t.output_tokens, 0);
        assert_eq!(t.dur, None, "缺列应按 null 降级");
        assert_eq!(t.ttft, None);
        assert_eq!(t.sub, None, "无 session 表时放弃并入");
        assert_eq!(t.models, "", "无 model_usage 表时无模型清单");
        assert!(orphans.is_empty(), "无子轮时无游离子代理完成轮");
        drop(conn);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn 有表时正常读出_主会话与子代理拆分() {
        // 完整 schema（与 v3.10.1 实测一致的关键列）+ 主/子代理行混合
        let (conn, path) = temp_db("full");
        conn.execute_batch(
            "CREATE TABLE session (id TEXT PRIMARY KEY, parent_id TEXT);
             CREATE TABLE turn_usage (
                session_id TEXT, turn_id TEXT, status TEXT, started_at INTEGER,
                completed_at INTEGER, duration_ms INTEGER,
                time_to_first_token_ms INTEGER, model_request_count INTEGER,
                model_retry_count INTEGER, tool_call_count INTEGER,
                input_tokens INTEGER, output_tokens INTEGER, reasoning_tokens INTEGER,
                cache_creation_input_tokens INTEGER, cache_read_input_tokens INTEGER);
             CREATE TABLE model_usage (turn_id TEXT, model_id TEXT, started_at INTEGER);",
        )
        .unwrap();
        conn.execute_batch(
            "INSERT INTO session VALUES ('sess_main', NULL), ('sess_subagent_agent_1', 'sess_main');
             INSERT INTO turn_usage VALUES
               ('sess_main', 'turn_m', 'completed', 1000, 5000, 4000, 900, 2, 0, 1, 100, 200, 5, 10, 50),
               ('sess_subagent_agent_1', 'turn_s', 'completed', 1100, 1500, 350, 100, 1, 0, 0, 30, 40, 0, 0, 20);
             INSERT INTO model_usage VALUES
               ('turn_m', 'GLM-5.3', 1000), ('turn_s', 'GLM-4.7', 1100);",
        )
        .unwrap();
        let (out, orphans) = collect_turns(&conn, 0).unwrap().expect("应能读出");
        // V20：主轮行 + 子代理自身视图行两行并存导出
        assert_eq!(out.len(), 2, "主轮行与子代理自身视图行应并存：{out:?}");
        let t = out.iter().find(|t| t.turn_id == "turn_m").unwrap();
        // 子代理已并入（30/40/20 + 自身 100/200/50）
        assert_eq!(t.input_tokens, 130);
        assert_eq!(t.output_tokens, 240);
        assert_eq!(t.cache_read, 70);
        assert_eq!(t.requests, 3);
        assert_eq!(t.subagent, None, "主轮行不带 subagent 标记");
        let sub = t.sub.as_ref().expect("应有子聚合");
        assert_eq!(sub.n, 1);
        // 模型清单：主轮 + 并入子轮去重
        assert_eq!(t.models, "GLM-5.3,GLM-4.7");
        // V20 自身视图行：sess 为子代理会话、数值为子轮自身口径（该库
        // 无 user_message_id 列 → umid 降级 null；dur/ttft 为子轮自身值）
        let sv = out
            .iter()
            .find(|t| t.turn_id == "turn_s")
            .expect("子代理自身视图行应导出");
        assert_eq!(sv.session_id, "sess_subagent_agent_1");
        assert_eq!(sv.subagent, Some(1));
        assert_eq!(sv.user_message_id, None);
        assert_eq!(sv.input_tokens, 30, "自身视图行数值应为子轮自身值");
        assert_eq!(sv.output_tokens, 40);
        assert_eq!(sv.cache_read, 20);
        assert_eq!(sv.requests, 1);
        assert_eq!(sv.sub, None, "自身视图行不携带并入聚合");
        assert_eq!(sv.dur, Some(350), "自身视图行 dur 应为子轮自身口径");
        assert_eq!(sv.ttft, Some(100), "自身视图行 ttft 应为子轮自身口径");
        assert_eq!(sv.models, "GLM-4.7", "自身视图行的模型清单为自己的");
        // 子轮已整轮并入 turns → 不应再分流到 runs 侧（防双计）
        assert!(orphans.is_empty(), "已并入 turns 的子轮不应进游离集合：{orphans:?}");
        drop(conn);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn 速度_每轮与会话级使用同一请求快照且最新无效会清空() {
        let (conn, path) = temp_db("speed-contract");
        conn.execute_batch(
            "CREATE TABLE turn_usage (
                session_id TEXT, turn_id TEXT, status TEXT, started_at INTEGER,
                completed_at INTEGER, duration_ms INTEGER,
                time_to_first_token_ms INTEGER);
             CREATE TABLE model_usage (
                session_id TEXT, turn_id TEXT, started_at INTEGER, status TEXT,
                input_tokens INTEGER, output_tokens INTEGER,
                cache_read_input_tokens INTEGER, first_token_at INTEGER,
                completed_at INTEGER, duration_ms INTEGER, model_id TEXT);",
        )
        .unwrap();
        conn.execute_batch(
            "INSERT INTO turn_usage VALUES
                ('sess_speed', 'turn_speed', 'completed', 1000, 3000, 500, 200);
             INSERT INTO model_usage VALUES
                ('sess_speed', 'turn_speed', 1000, 'completed', 10, 100, 4,
                 2000, 3000, 2000, 'M');",
        )
        .unwrap();

        let (mut turns, _) = collect_turns_with_speed(&conn, 0)
            .unwrap()
            .expect("turn export");
        let turn_speed = turns[0].speed.clone().expect("turn speed");
        assert_eq!(turn_speed.quality, crate::token_speed::SpeedQuality::Generation);
        assert!((turn_speed.value - 100.0).abs() < f64::EPSILON);
        let session_speed = collect_session_stats(&conn, &turns, &[])
            .unwrap()
            .remove(0)
            .speed
            .expect("session speed");
        assert_eq!(session_speed, turn_speed, "两出口应复用同一请求级快照");

        // The newer row is invalid. It must win selection and clear the speed
        // rather than allowing the older valid request to remain visible.
        conn.execute(
            "INSERT INTO model_usage VALUES
                ('sess_speed', 'turn_speed', 2000, 'completed', 10, 50, 0,
                 4000, 3500, 1500, 'M')",
            [],
        )
        .unwrap();
        turns = collect_turns_with_speed(&conn, 0)
            .unwrap()
            .expect("turn export")
            .0;
        assert!(turns[0].speed.is_none(), "最新无效请求不得沿用旧速度");
        let session = collect_session_stats(&conn, &turns, &[]).unwrap();
        assert!(session[0].speed.is_none(), "会话级也应清空旧速度");

        // A newer in-flight request must suppress the previous completed
        // request as well; skipping non-terminal rows would leak the old
        // speed through the one-second side refresh.
        conn.execute(
            "INSERT INTO model_usage VALUES
                ('sess_speed', 'turn_speed', 3000, 'running', 10, 500, 0,
                 NULL, NULL, NULL, 'M')",
            [],
        )
        .unwrap();
        turns = collect_turns_with_speed(&conn, 0)
            .unwrap()
            .expect("turn export")
            .0;
        assert!(turns[0].speed.is_none(), "进行中最新请求不得沿用旧速度");

        drop(conn);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn 速度查询_联合索引计划与单次目录读取() {
        use std::time::Instant;

        let (conn, path) = temp_db("speed-query-plan");
        conn.execute_batch(
            "CREATE TABLE model_usage (
                session_id TEXT, turn_id TEXT, started_at INTEGER, status TEXT,
                output_tokens INTEGER, first_token_at INTEGER, completed_at INTEGER,
                duration_ms INTEGER);
             CREATE INDEX model_usage_session_turn_idx
                ON model_usage(session_id, turn_id);",
        )
        .unwrap();
        conn.execute_batch("BEGIN").unwrap();
        for session in 0..240 {
            for turn in 0..240 {
                conn.execute(
                    "INSERT INTO model_usage VALUES (?1, ?2, ?3, 'completed', 100, ?4, ?5, 1000)",
                    rusqlite::params![
                        format!("sess_{session}"),
                        format!("turn_{turn}"),
                        1_000 + session * 100 + turn,
                        1_500 + session * 100 + turn,
                        2_000 + session * 100 + turn,
                    ],
                )
                .unwrap();
            }
        }
        conn.execute_batch("COMMIT").unwrap();

        let explain = |sql: &str| -> Vec<String> {
            conn.prepare(sql)
                .unwrap()
                .query_map(rusqlite::params!["turn_1", "turn_2"], |row| {
                    row.get::<_, String>(3)
                })
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
        };
        let old_plan = explain(
            "EXPLAIN QUERY PLAN SELECT session_id, turn_id FROM model_usage \
             WHERE turn_id IN (?1, ?2)",
        );
        let new_plan = explain(
            "EXPLAIN QUERY PLAN SELECT session_id, turn_id FROM model_usage \
             WHERE session_id IN (?1, ?2)",
        );
        eprintln!("[speed-query-plan] old={old_plan:?} new={new_plan:?}");
        assert!(
            old_plan.iter().any(|detail| detail.contains("SCAN model_usage")),
            "未带 session 前缀的旧查询不应伪装成索引点查: {old_plan:?}"
        );
        assert!(
            new_plan.iter().any(|detail| {
                detail.contains("INDEX model_usage_session_turn_idx")
                    && detail.contains("session_id=?")
            }),
            "新查询应使用(session_id, turn_id)索引前缀: {new_plan:?}"
        );

        let sessions = ["sess_1".to_string(), "sess_2".to_string()]
            .into_iter()
            .collect::<BTreeSet<_>>();
        let old_started = Instant::now();
        for _ in 0..20 {
            // 旧导出周期会为 turns、runs、sess 分别读取同一张表；这里
            // 以 3 次原 bare turn_id 查询模拟其重复成本，并读同一批
            // 请求级字段，避免只比较一条极简 SELECT 的偏差。
            for _ in 0..3 {
                let mut stmt = conn
                    .prepare(
                        "SELECT session_id, turn_id, output_tokens, started_at, \
                         first_token_at, completed_at, duration_ms, status \
                         FROM model_usage WHERE turn_id IN (?1, ?2)",
                    )
                    .unwrap();
                let mut rows = stmt
                    .query(rusqlite::params!["turn_1", "turn_2"])
                    .unwrap();
                while rows.next().unwrap().is_some() {}
            }
        }
        let old_elapsed = old_started.elapsed();
        let new_started = Instant::now();
        for _ in 0..20 {
            let _ = SpeedCatalog::load(&conn, &sessions).unwrap();
        }
        let new_elapsed = new_started.elapsed();
        eprintln!(
            "[speed-query-plan] 20x elapsed old={old_elapsed:?} new={new_elapsed:?}"
        );
        assert!(new_elapsed < old_elapsed, "联合索引查询应明显少于全表扫");

        drop(conn);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn sess序列化_键名契约与全零形态() {
        // 短键名一字不差（usage.js 按名消费）；V22 起 cp/cu/cw 已随 CTX
        // 展示下线删除，不再出现在序列化输出
        let row = UsageSessionStat {
            session_id: "sess_1".to_string(),
            total: 1110,
            plain_in: 100,
            out_tokens: 200,
            cache_read: 50,
            requests: 2,
            speed: None,
            speed_state: SpeedState::Unavailable,
            member_session_ids: vec!["sess_1".to_string()],
            round_started_at: None,
            generating: false,
        };
        let json = serde_json::to_string(&vec![row]).unwrap();
        assert_eq!(
            json,
            "[{\"s\":\"sess_1\",\"tt\":1110,\"up\":100,\"down\":200,\"cr\":50,\"rq\":2,\"speedState\":\"unavailable\"}]",
            "sess 行序列化形态不符：{json}"
        );
        // 全零行（无任何请求）：各合计为 0
        let none_row = UsageSessionStat {
            session_id: "sess_2".to_string(),
            total: 0,
            plain_in: 0,
            out_tokens: 0,
            cache_read: 0,
            requests: 0,
            speed: None,
            speed_state: SpeedState::Unavailable,
            member_session_ids: vec!["sess_2".to_string()],
            round_started_at: None,
            generating: false,
        };
        let json = serde_json::to_string(&vec![none_row]).unwrap();
        assert_eq!(
            json,
            "[{\"s\":\"sess_2\",\"tt\":0,\"up\":0,\"down\":0,\"cr\":0,\"rq\":0,\"speedState\":\"unavailable\"}]",
            "{json}"
        );
        // CTX 短键零残留（V22 删除 cp/cu/cw）
        assert!(!json.contains("\"cp\"") && !json.contains("\"cu\"") && !json.contains("\"cw\""));
    }

    #[test]
    fn sess聚合_会话树全量合计() {
        // 库形态：主会话 + 两个子代理（其一嵌套更深），含失败轮请求行
        //（turn_usage 无行）与 turn_id NULL 的 CLI 后台请求行（不过滤，
        // 计入全量合计）
        let (conn, path) = temp_db("sess-agg");
        conn.execute_batch(
            "CREATE TABLE session (id TEXT PRIMARY KEY, parent_id TEXT);
             CREATE TABLE turn_usage (
                session_id TEXT, turn_id TEXT, status TEXT, started_at INTEGER);
             CREATE TABLE model_usage (
                session_id TEXT, turn_id TEXT, started_at INTEGER, model_id TEXT,
                status TEXT, input_tokens INTEGER, output_tokens INTEGER,
                cache_read_input_tokens INTEGER);",
        )
        .unwrap();
        conn.execute_batch(
            "INSERT INTO session VALUES
               ('sess_main', NULL), ('sess_sub1', 'sess_main'),
               ('sess_sub2', 'sess_main'), ('sess_sub2_child', 'sess_sub2');
             INSERT INTO model_usage VALUES
               -- 主会话：完成请求 + 失败请求（无 turn_usage 行，旧口径漏计）
               ('sess_main', 't1', 1000, 'GLM-4.6', 'completed', 100, 200, 40),
               ('sess_main', 't2', 2000, 'GLM-4.6', 'error', 50, 0, 0),
               -- turn_id NULL 的后台请求（session_title 等）：计入合计
               ('sess_main', NULL, 1500, 'GLM-4.6', 'completed', 10, 5, 0),
               -- 子代理 1：完成请求（started_at 最大）
               ('sess_sub1', 't3', 5000, 'GLM-5.3', 'completed', 13107, 300, 100),
               -- 子代理 2 及其嵌套子代理
               ('sess_sub2', 't4', 3000, 'GLM-4.7', 'completed', 30, 40, 20),
               ('sess_sub2_child', 't5', 3500, 'GLM-4.7', 'completed', 30, 10, 10);",
        )
        .unwrap();
        // 出现过的会话：turns 携带 sess_main（模拟 turns/runs 的出现集合）
        let turns = vec![turn("t1", "sess_main", 1000, Some(2000))];
        let runs: Vec<UsageRun> = Vec::new();
        let sess = collect_session_stats(&conn, &turns, &runs).unwrap();

        // 目标 = 出现的 sess_main（根），子代理作为其后代并入；子代理
        // 未出现在 turns/runs → 不单独导出行
        assert_eq!(sess.len(), 1, "{sess:?}");
        let main = &sess[0];
        assert_eq!(main.session_id, "sess_main");
        // 全量合计：↑ = 60 + 50 + 10 + 13007 + 10 + 20 = 13157
        assert_eq!(main.plain_in, 60 + 50 + 10 + (13107 - 100) + 10 + 20);
        // ↓ = 200 + 0 + 5 + 300 + 40 + 10
        assert_eq!(main.out_tokens, 555);
        // ⟲ = 40 + 100 + 20 + 10
        assert_eq!(main.cache_read, 170);
        assert_eq!(main.requests, 6, "失败轮与 NULL turn_id 行均计入");
        assert_eq!(main.total, main.plain_in + main.out_tokens + main.cache_read);
        drop(conn);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn sess聚合_子代理出现导出自身树与根会话行() {
        // 子代理自身出现在 runs（主轮静默场景）：导出根会话行（树合计）
        // + 子代理自身树行；两行各自查询互不重叠
        let (conn, path) = temp_db("sess-self");
        conn.execute_batch(
            "CREATE TABLE session (id TEXT PRIMARY KEY, parent_id TEXT);
             CREATE TABLE turn_usage (
                session_id TEXT, turn_id TEXT, status TEXT, started_at INTEGER);
             CREATE TABLE model_usage (
                session_id TEXT, turn_id TEXT, started_at INTEGER, model_id TEXT,
                status TEXT, input_tokens INTEGER, output_tokens INTEGER,
                cache_read_input_tokens INTEGER);",
        )
        .unwrap();
        conn.execute_batch(
            "INSERT INTO session VALUES
               ('sess_main', NULL), ('sess_sub1', 'sess_main');
             INSERT INTO model_usage VALUES
               ('sess_main', 't1', 1000, 'GLM-4.6', 'completed', 100, 200, 40),
               ('sess_sub1', 't3', 5000, 'GLM-5.3', 'completed', 30, 40, 20);",
        )
        .unwrap();
        // 仅子代理出现（主轮未落库、turns 空）：目标 = sess_sub1 + 其根
        // sess_main（上溯补入），主会话行也导出
        let runs = vec![UsageRun {
            turn_id: "turn_sub_run".to_string(),
            user_message_id: Some("msg_c".to_string()),
            session_id: "sess_sub1".to_string(),
            parent_session_id: Some("sess_main".to_string()),
            input_tokens: 30,
            output_tokens: 40,
            cache_read: 20,
            cache_write: 0,
            reasoning: 0,
            requests: 1,
            start: 5000,
            merged: None,
            sub: None,
            speed: None,
        }];
        let sess = collect_session_stats(&conn, &[], &runs).unwrap();
        assert_eq!(sess.len(), 2, "{sess:?}");
        let main = sess.iter().find(|s| s.session_id == "sess_main").unwrap();
        let sub = sess.iter().find(|s| s.session_id == "sess_sub1").unwrap();
        // 主会话树行含子代理（↑ = 60 + 10）
        assert_eq!(main.plain_in, 70);
        assert_eq!(main.requests, 2);
        // 子代理自身树行 = 仅自身（↑ = 10）
        assert_eq!(sub.plain_in, 10);
        assert_eq!(sub.requests, 1);
        drop(conn);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn sess聚合_首请求未完成时由待处理用户消息带出历史全生命周期累计() {
        let (conn, path) = temp_db("sess-pending-lifetime");
        conn.execute_batch(
            "CREATE TABLE session (id TEXT PRIMARY KEY, parent_id TEXT);
             CREATE TABLE message (
                id TEXT PRIMARY KEY, session_id TEXT, time_created INTEGER, data TEXT);
             CREATE TABLE model_usage (
                session_id TEXT, turn_id TEXT, started_at INTEGER, status TEXT,
                input_tokens INTEGER, output_tokens INTEGER,
                cache_read_input_tokens INTEGER, first_token_at INTEGER,
                completed_at INTEGER);
             INSERT INTO session VALUES ('sess_old_active', NULL);
             INSERT INTO model_usage VALUES
                ('sess_old_active', 'old_turn', 1000, 'completed', 1000, 2000, 300, 1100, 1200);
             INSERT INTO message VALUES
                ('msg_new_active', 'sess_old_active', 9999999900, '{\"role\":\"user\"}');",
        )
        .unwrap();
        let now = 9_999_999_900 + 1_000;

        // 没有 turns/runs：首笔请求尚未完成，只有发送即落库的 user message。
        let pending = collect_pending_user(&conn, &BTreeSet::new(), now, &BTreeSet::new())
            .unwrap()
            .expect("应识别待处理用户消息");
        assert_eq!(pending.session_id, "sess_old_active");
        let ids = session_ids_for_stats(&conn, &[], &[], Some(&pending.session_id)).unwrap();
        let catalog = SpeedCatalog::load(&conn, &ids).unwrap();
        let stats = collect_session_stats_with_catalog(
            &conn,
            &[],
            &[],
            &catalog,
            Some(&pending),
        )
        .unwrap();
        assert_eq!(stats.len(), 1);
        let active = &stats[0];
        assert_eq!(active.session_id, "sess_old_active");
        assert_eq!(active.plain_in, 700, "历史累计不应在首请求阶段归零");
        assert_eq!(active.out_tokens, 2000);
        assert_eq!(active.cache_read, 300);
        assert_eq!(active.requests, 1);
        assert_eq!(active.total, 3000);
        // A1/A2：首请求等待期为 measuring 且 generating 独立成立；本轮
        // 边界排除旧请求速度
        assert!(active.generating, "新鲜待处理消息应视为生成中");
        assert_eq!(active.speed_state, SpeedState::Measuring);
        assert_eq!(active.speed, None, "新轮不得回填旧轮速度");

        // A1 孤儿超时：同一消息超过新鲜期且无活跃佐证 → 不再冒充待处理，
        // 会话回到空闲。纯 pending 通道的会话随之退出 sess 导出（渲染端回
        // 退 turns 口径）；这里补一条完成轮让会话仍出现在 turns，验证空闲
        // 状态与累计不清零
        let stale_now = 9_999_999_900 + PENDING_FRESH_MS + 1;
        let dropped = collect_pending_user(
            &conn,
            &BTreeSet::new(),
            stale_now,
            &BTreeSet::new(),
        )
        .unwrap();
        assert_eq!(dropped, None, "超期孤儿消息应被清掉");
        let turns = vec![turn("old_turn", "sess_old_active", 1000, Some(1200))];
        let stats = collect_session_stats_with_catalog(&conn, &turns, &[], &catalog, dropped.as_ref())
            .unwrap();
        assert_eq!(stats.len(), 1);
        let idle = &stats[0];
        assert!(!idle.generating);
        assert_eq!(idle.speed_state, SpeedState::Recent, "空闲会话显示最近确认速度");
        // 历史累计不被陈旧状态清理清零（model_usage 全量口径，非 turns 合计）
        assert_eq!(idle.plain_in, 700);
        assert_eq!(idle.total, 3000);

        drop(conn);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn sess聚合_多层子代理佐证与同轮多请求状态() {
        // A1 多层子代理：主会话待处理消息超期，但树内子代理（含二层）仍
        // 有进行中轮（runs 佐证）→ 会话保持生成中，累计含全层子代理。
        // A2：同轮第一请求完成（有速度）后第二请求进行中（runs 仍在）→
        // generating=true 且 Recent；速度暂时无效时回落 Measuring。
        let (conn, path) = temp_db("sess-tree-corroborate");
        conn.execute_batch(
            "CREATE TABLE session (id TEXT PRIMARY KEY, parent_id TEXT);
             CREATE TABLE message (
                id TEXT PRIMARY KEY, session_id TEXT, time_created INTEGER, data TEXT);
             CREATE TABLE model_usage (
                session_id TEXT, turn_id TEXT, started_at INTEGER, status TEXT,
                input_tokens INTEGER, output_tokens INTEGER,
                cache_read_input_tokens INTEGER, first_token_at INTEGER,
                completed_at INTEGER, duration_ms INTEGER);
             INSERT INTO session VALUES
               ('sess_main', NULL),
               ('sess_sub1', 'sess_main'),
               ('sess_sub2_child', 'sess_sub1');
             INSERT INTO model_usage VALUES
               ('sess_main', 't_old', 1000, 'completed', 100, 200, 30, 1100, 1200, 1200),
               ('sess_sub2_child', 't_live', 9900000, 'completed', 40, 80, 0, 9900100, 9900200, 200);
             INSERT INTO message VALUES
               ('msg_stale_main', 'sess_main', 9_000, '{\"role\":\"user\"}');",
        )
        .unwrap();
        let now = 9_999_999;
        // 超期主会话消息 + 二层子代理进行中轮（runs 形态）→ 佐证成立
        let corroborate: BTreeSet<String> = ["sess_main".to_string()].into_iter().collect();
        let pending = collect_pending_user(&conn, &BTreeSet::new(), now, &corroborate)
            .unwrap()
            .expect("树内活跃佐证应保留超期待处理消息");
        assert_eq!(pending.session_id, "sess_main");

        // 模拟 runs：二层子代理的进行中轮（turn_usage 无行）
        let runs = vec![UsageRun {
            turn_id: "t_live".to_string(),
            user_message_id: None,
            session_id: "sess_sub2_child".to_string(),
            parent_session_id: Some("sess_sub1".to_string()),
            input_tokens: 40,
            output_tokens: 80,
            cache_read: 0,
            cache_write: 0,
            reasoning: 0,
            requests: 1,
            start: 9_900_000,
            merged: None,
            sub: None,
            speed: Some(SpeedSnapshot {
                value: 800.0,
                quality: SpeedQuality::Generation,
                completed_at: 9_900_200,
                request_id: None,
            }),
        }];
        let ids = session_ids_for_stats(&conn, &[], &runs, None).unwrap();
        let catalog = SpeedCatalog::load(&conn, &ids).unwrap();
        let stats =
            collect_session_stats_with_catalog(&conn, &[], &runs, &catalog, Some(&pending))
                .unwrap();
        let main = stats
            .iter()
            .find(|s| s.session_id == "sess_main")
            .expect("根会话行");
        // 全层子代理累计：↑ = (100-30) + (40-0) = 110，↓ = 280
        assert_eq!(main.plain_in, 110);
        assert_eq!(main.out_tokens, 280);
        // 同轮有已完成请求（runs 带速度）→ generating + Recent
        assert!(main.generating);
        assert_eq!(main.speed_state, SpeedState::Recent);

        // A2 回落：同轮第二请求进行中（目录最新行无效）→ 速度清空且状态
        // 回落 Measuring，绝不因"没有速度"转成 Unavailable
        conn.execute(
            "INSERT INTO model_usage VALUES
               ('sess_sub2_child', 't_live', 9950000, 'running', 10, 500, 0, NULL, NULL, NULL)",
            [],
        )
        .unwrap();
        let ids = session_ids_for_stats(&conn, &[], &runs, None).unwrap();
        let catalog = SpeedCatalog::load(&conn, &ids).unwrap();
        let stats =
            collect_session_stats_with_catalog(&conn, &[], &runs, &catalog, Some(&pending))
                .unwrap();
        let main = stats.iter().find(|s| s.session_id == "sess_main").unwrap();
        assert!(main.generating, "第二请求进行中仍视为生成中");
        assert_eq!(main.speed, None, "最新请求无效时不得沿用旧速度");
        assert_eq!(
            main.speed_state,
            SpeedState::Measuring,
            "生成中速度暂时无效应回落 measuring"
        );
        drop(conn);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn sess聚合_无session表降级为各会话自身合计() {
        // 老版本库：无 session 表 → 无归并，出现的会话各自聚合自身；
        // status 列同库缺失（本用例顺带覆盖，合计口径不区分状态）
        let (conn, path) = temp_db("sess-nosession");
        conn.execute_batch(
            "CREATE TABLE turn_usage (
                session_id TEXT, turn_id TEXT, status TEXT, started_at INTEGER);
             CREATE TABLE model_usage (
                session_id TEXT, turn_id TEXT, started_at INTEGER, model_id TEXT,
                input_tokens INTEGER, output_tokens INTEGER,
                cache_read_input_tokens INTEGER);",
        )
        .unwrap();
        conn.execute_batch(
            "INSERT INTO model_usage VALUES
               ('sess_a', 't1', 1000, 'GLM-4.6', 100, 200, 40),
               ('sess_b', 't2', 2000, 'GLM-5.3', 50, 60, 10);",
        )
        .unwrap();
        let turns = vec![
            turn("t1", "sess_a", 1000, Some(2000)),
            turn("t2", "sess_b", 2000, Some(3000)),
        ];
        let sess = collect_session_stats(&conn, &turns, &[]).unwrap();
        assert_eq!(sess.len(), 2, "{sess:?}");
        let a = sess.iter().find(|s| s.session_id == "sess_a").unwrap();
        assert_eq!(a.plain_in, 60, "无 session 表不归并，仅自身");
        assert_eq!(a.requests, 1);
        drop(conn);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn 导出窗口与上限常量_7天3000轮() {
        assert_eq!(WINDOW_MS, 7 * 24 * 3600 * 1000, "导出窗口应为 7 天");
        assert_eq!(MAX_TURNS, 3000, "导出行数上限应为 3000 轮");
        assert_eq!(RUN_WINDOW_MS, 10 * 60 * 1000, "runs 新鲜度窗口应为 10 分钟");
        assert_eq!(PENDING_SCAN_ROWS, 64, "pu 尾部扫查应为 64 行（实测见模块头）");
        assert_eq!(TOOL_WINDOW_MS, 10 * 60 * 1000, "ta 窗口应为 10 分钟（崩溃残留兜底）");
    }

    /// pu 查询测试库（message 表按真实 schema 最小化）
    fn pu_db(name: &str) -> (Connection, std::path::PathBuf) {
        let (conn, path) = temp_db(name);
        conn.execute_batch(
            "CREATE TABLE message (
                id TEXT PRIMARY KEY, session_id TEXT, time_created INTEGER,
                time_updated INTEGER, data TEXT, sequence INTEGER);",
        )
        .unwrap();
        (conn, path)
    }

    #[test]
    fn pu查询_user消息识别与完成轮匹配() {
        let (conn, path) = pu_db("pu-basic");
        // 固定观测时刻：全部样本都在新鲜期内（A1 之前的行为不受影响）
        let now = 5_000i64;
        // 空表 → 无待处理消息
        assert_eq!(
            collect_pending_user_ms(&conn, &BTreeSet::new(), now, &BTreeSet::new()).unwrap(),
            None,
            "空 message 表应返回 None"
        );
        // 混合消息：已完成 user（older）、assistant、待处理 user（newer）、
        // 子代理会话 user（同样参与——子代理任务派发也是"工作将开始"）
        conn.execute_batch(
            "INSERT INTO message VALUES
               ('msg_done', 'sess_1', 1000, 1000, '{\"role\":\"user\",\"agent\":\"zcode-agent\"}', 1),
               ('msg_a1', 'sess_1', 1500, 1500, '{\"role\":\"assistant\"}', 2),
               ('msg_new', 'sess_1', 3000, 3000, '{\"role\":\"user\",\"agent\":\"zcode-agent\"}', 3),
               ('msg_sub', 'sess_subagent_x', 3500, 3500, '{\"role\":\"user\",\"agent\":\"zcode-agent\"}', 4),
               ('msg_a2', 'sess_1', 4000, 4000, '{\"role\":\"assistant\"}', 5);",
        )
        .unwrap();
        // 全空匹配集：最近 user（msg_sub 3500，跳过 assistant）待处理 → 3500
        assert_eq!(
            collect_pending_user_ms(&conn, &BTreeSet::new(), now, &BTreeSet::new()).unwrap(),
            Some(3500),
            "最近的未匹配 user 消息应透出（子代理会话同样参与）"
        );
        // msg_sub 已完成（umid 集合含它）→ 次新未匹配 msg_new → 3000
        let done: BTreeSet<String> = ["msg_sub".to_string()].into_iter().collect();
        assert_eq!(
            collect_pending_user_ms(&conn, &done, now, &BTreeSet::new()).unwrap(),
            Some(3000),
            "已完成的 user 消息应被跳过，取次新的待处理消息"
        );
        // 全部 user 消息均已完成 → None
        let done_all: BTreeSet<String> =
            ["msg_sub".to_string(), "msg_new".to_string(), "msg_done".to_string()]
                .into_iter()
                .collect();
        assert_eq!(
            collect_pending_user_ms(&conn, &done_all, now, &BTreeSet::new()).unwrap(),
            None,
            "全部 user 消息均有完成轮时应返回 None"
        );
        // 最新 user 已完成但更早的未匹配：按 time 降序取第一条未匹配
        let done_new_only: BTreeSet<String> = ["msg_sub".to_string(), "msg_new".to_string()]
            .into_iter()
            .collect();
        assert_eq!(
            collect_pending_user_ms(&conn, &done_new_only, now, &BTreeSet::new()).unwrap(),
            Some(1000),
            "应返回 time 降序第一条未匹配的 user 消息"
        );
        drop(conn);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn pu查询_a1新鲜期与活跃佐证() {
        let (conn, path) = pu_db("pu-fresh");
        // 孤儿消息：12 小时前未匹配、无任何活跃证据 → 不再冒充待处理
        let stale = 5_000_000_000_i64 - 12 * 3600 * 1000;
        conn.execute_batch(&format!(
            "INSERT INTO message VALUES
               ('msg_stale', 'sess_orphan', {stale}, {stale}, '{{\"role\":\"user\"}}', 1);",
        ))
        .unwrap();
        assert_eq!(
            collect_pending_user_ms(
                &conn,
                &BTreeSet::new(),
                5_000_000_000,
                &BTreeSet::new()
            )
            .unwrap(),
            None,
            "超出新鲜期且无活跃佐证的孤儿消息不透出"
        );
        // 同一条消息，会话树内有进行中轮（佐证集合含其会话）→ 保留等待
        let corroborate: BTreeSet<String> =
            ["sess_orphan".to_string()].into_iter().collect();
        assert_eq!(
            collect_pending_user_ms(&conn, &BTreeSet::new(), 5_000_000_000, &corroborate)
                .unwrap(),
            Some(stale),
            "有活跃佐证的超期长首请求仍保留等待"
        );

        // 新消息到达（新一轮）：更新者胜出，陈旧孤儿立即被替换
        conn.execute_batch(
            "INSERT INTO message VALUES
               ('msg_new_round', 'sess_orphan', 4999999900, 4999999900, '{\"role\":\"user\"}', 2);",
        )
        .unwrap();
        assert_eq!(
            collect_pending_user_ms(&conn, &BTreeSet::new(), 5_000_000_000, &BTreeSet::new())
                .unwrap(),
            Some(4_999_999_900),
            "新一轮消息应立即替换旧等待"
        );

        // 取消/失败：turn_usage 落 cancelled 轮（umid 匹配）→ 新消息被
        // 完成集合清掉后，旧孤儿仍因超期无佐证不透出
        conn.execute_batch(
            "CREATE TABLE turn_usage (session_id TEXT, turn_id TEXT, status TEXT,
                started_at INTEGER, completed_at INTEGER, user_message_id TEXT);
             INSERT INTO turn_usage VALUES
               ('sess_orphan', 'turn_cancel', 'cancelled', 4999999800, 4999999950, 'msg_new_round');",
        )
        .unwrap();
        // 与 collect_turns 同口径：完成集合由调用方从 turns umid 聚合，这里
        // 直接以查得形态模拟（turn_usage.user_message_id 进集合）
        let done: BTreeSet<String> = ["msg_new_round".to_string()].into_iter().collect();
        assert_eq!(
            collect_pending_user_ms(&conn, &done, 5_000_000_000, &BTreeSet::new()).unwrap(),
            None,
            "取消轮清掉新等待后，超期孤儿不得回填"
        );
        drop(conn);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn pu查询_坏数据与超窗降级() {
        let (conn, path) = pu_db("pu-degrade");
        // 坏数据防御：data 缺 role 键 / 非法 JSON 由 json_extract 返回
        // NULL 过滤；time_created = 0 的脏行不透出
        conn.execute_batch(
            "INSERT INTO message VALUES
               ('msg_bad0', 'sess_1', 0, 0, '{\"role\":\"user\"}', 1),
               ('msg_norole', 'sess_1', 500, 500, '{\"time\":{\"created\":500}}', 2),
               ('msg_ok', 'sess_1', 800, 800, '{\"role\":\"user\"}', 3);",
        )
        .unwrap();
        assert_eq!(
            collect_pending_user_ms(&conn, &BTreeSet::new(), 5_000, &BTreeSet::new()).unwrap(),
            Some(800),
            "缺 role 的消息与脏时刻行应被跳过"
        );
        drop(conn);
        let _ = fs::remove_file(&path);

        // 扫查窗口：待处理 user 消息被 64 行 assistant 消息挤出尾部 →
        // 不透出（尾部扫查窗口语义；活跃期 user 消息密度远高于此）
        let (conn, path) = pu_db("pu-window");
        conn.execute_batch(
            "INSERT INTO message VALUES
               ('msg_old_user', 'sess_1', 100, 100, '{\"role\":\"user\"}', 1);",
        )
        .unwrap();
        for i in 0..PENDING_SCAN_ROWS {
            conn.execute(
                "INSERT INTO message VALUES (?1, 'sess_1', ?2, ?2, '{\"role\":\"assistant\"}', ?3)",
                rusqlite::params![format!("msg_a{i}"), 200 + i, i + 2],
            )
            .unwrap();
        }
        assert_eq!(
            collect_pending_user_ms(&conn, &BTreeSet::new(), 500, &BTreeSet::new()).unwrap(),
            None,
            "被尾部 64 行挤出的待处理消息不透出（窗口兜底，陈旧信号不放大）"
        );
        drop(conn);
        let _ = fs::remove_file(&path);

        // 无 message 表（老版本库）→ None（pu 信号整体缺失，不报错）
        let (conn, path) = temp_db("pu-no-table");
        conn.execute_batch("CREATE TABLE turn_usage (session_id TEXT);")
            .unwrap();
        assert_eq!(
            collect_pending_user_ms(&conn, &BTreeSet::new(), 0, &BTreeSet::new()).unwrap(),
            None,
            "无 message 表应返回 None"
        );
        drop(conn);
        let _ = fs::remove_file(&path);

        // message 表缺核心列（极端形态）→ None
        let (conn, path) = temp_db("pu-missing-col");
        conn.execute_batch("CREATE TABLE message (id TEXT PRIMARY KEY, data TEXT);")
            .unwrap();
        assert_eq!(
            collect_pending_user_ms(&conn, &BTreeSet::new(), 0, &BTreeSet::new()).unwrap(),
            None,
            "缺 time_created 核心列应返回 None"
        );
        drop(conn);
        let _ = fs::remove_file(&path);
    }

    /// ta/fe 查询测试库（tool_usage + turn_usage 按真实 schema 最小化）
    fn ta_fe_db(name: &str) -> (Connection, std::path::PathBuf) {
        let (conn, path) = temp_db(name);
        conn.execute_batch(
            "CREATE TABLE tool_usage (
                id TEXT PRIMARY KEY, session_id TEXT NOT NULL, tool_name TEXT NOT NULL,
                status TEXT NOT NULL, started_at INTEGER NOT NULL,
                completed_at INTEGER, exit_code INTEGER,
                cancelled_by_user INTEGER NOT NULL DEFAULT 0);
             CREATE TABLE turn_usage (
                session_id TEXT, turn_id TEXT, status TEXT, started_at INTEGER,
                completed_at INTEGER, cancelled_by_user INTEGER,
                tool_error_count INTEGER);",
        )
        .unwrap();
        (conn, path)
    }

    #[test]
    fn ta查询_running行透出与窗口残留剔除() {
        let (conn, path) = ta_fe_db("ta");
        let now = 4_000_000_000_i64;
        // 空表 → 无活跃工具
        assert_eq!(
            collect_active_tool_ms(&conn, now - TOOL_WINDOW_MS).unwrap(),
            None,
            "空 tool_usage 表应返回 None"
        );
        // 混合行：完成行（新）、running 行（窗口内）、崩溃残留 running 行
        //（20 分钟前，窗口外）、脏 started_at=0 的 running 行
        conn.execute_batch(&format!(
            "INSERT INTO tool_usage (id, session_id, tool_name, status, started_at, completed_at) VALUES
               ('t_done', 's1', 'Bash', 'completed', {c1}, {c2}),
               ('t_run', 's1', 'Read', 'running', {r1}, NULL),
               ('t_run2', 's2', 'Write', 'running', {r2}, NULL),
               ('t_stale', 's3', 'Bash', 'running', {st}, NULL),
               ('t_zero', 's4', 'Bash', 'running', 0, NULL);",
            c1 = now - 30_000,
            c2 = now - 20_000,
            r1 = now - 10_000,
            r2 = now - 5_000,
            st = now - 20 * 60 * 1000,
        ))
        .unwrap();
        // ta = 窗口内最新 running 行的 started_at（完成行不参与；残留与
        // 脏行被窗口/值防御剔除）
        assert_eq!(
            collect_active_tool_ms(&conn, now - TOOL_WINDOW_MS).unwrap(),
            Some(now - 5_000),
            "ta 应取窗口内最新 running 行的 started_at"
        );
        // 工具全部完成 → running 行消失 → ta 归 null（正常结束自愈路径）
        conn.execute_batch(&format!(
            "UPDATE tool_usage SET status = 'completed', completed_at = {c} WHERE status = 'running';",
            c = now - 1_000,
        ))
        .unwrap();
        assert_eq!(
            collect_active_tool_ms(&conn, now - TOOL_WINDOW_MS).unwrap(),
            None,
            "running 行全部完成后 ta 应归 null"
        );
        drop(conn);
        let _ = fs::remove_file(&path);

        // 降级：无 tool_usage 表（老版本库）→ None 不报错
        let (conn, path) = temp_db("ta-no-table");
        conn.execute_batch("CREATE TABLE turn_usage (session_id TEXT);")
            .unwrap();
        assert_eq!(
            collect_active_tool_ms(&conn, 0).unwrap(),
            None,
            "无 tool_usage 表应返回 None"
        );
        drop(conn);
        let _ = fs::remove_file(&path);

        // 降级：缺核心列（极端形态）→ None
        let (conn, path) = temp_db("ta-missing-col");
        conn.execute_batch("CREATE TABLE tool_usage (id TEXT PRIMARY KEY, status TEXT);")
            .unwrap();
        assert_eq!(
            collect_active_tool_ms(&conn, 0).unwrap(),
            None,
            "缺 started_at 核心列应返回 None"
        );
        drop(conn);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn fe查询_失败轮判定与成功轮不刷新() {
        let (conn, path) = ta_fe_db("fe");
        let now = 5_000_000_000_i64;
        // 空表 → 无失败事件
        assert_eq!(
            collect_failure_event_ms(&conn, now - WINDOW_MS).unwrap(),
            None,
            "空 turn_usage 表应返回 None"
        );
        // 场景：历史失败轮（10 分钟前完成）+ 成功轮（1 分钟前完成）——
        // fe 只在失败轮新增时变化：成功轮不刷新，fe 停在失败轮完成时刻
        conn.execute_batch(&format!(
            "INSERT INTO turn_usage VALUES
               ('s1', 'turn_fail', 'error', {f0}, {f1}, 0, 0),
               ('s1', 'turn_ok', 'completed', {o0}, {o1}, 0, 0);",
            f0 = now - 700_000,
            f1 = now - 600_000,
            o0 = now - 100_000,
            o1 = now - 60_000,
        ))
        .unwrap();
        assert_eq!(
            collect_failure_event_ms(&conn, now - WINDOW_MS).unwrap(),
            Some(now - 600_000),
            "成功轮完成不应刷新 fe（MAX 只取失败轮完成时刻）"
        );
        // 三种失败形态全命中：cancelled 状态 / 用户取消标记 / 工具报错
        conn.execute_batch(&format!(
            "INSERT INTO turn_usage VALUES
               ('s1', 'turn_cancel', 'cancelled', {c0}, {c1}, 0, 0),
               ('s1', 'turn_ubye', 'completed', {u0}, {u1}, 1, 0),
               ('s1', 'turn_toolerr', 'completed', {t0}, {t1}, 0, 2);",
            c0 = now - 50_000,
            c1 = now - 40_000,
            u0 = now - 30_000,
            u1 = now - 25_000,
            t0 = now - 20_000,
            t1 = now - 10_000,
        ))
        .unwrap();
        assert_eq!(
            collect_failure_event_ms(&conn, now - WINDOW_MS).unwrap(),
            Some(now - 10_000),
            "cancelled/用户取消/工具报错均应命中，取最新完成时刻"
        );
        // 窗口过滤：失败轮滑出窗口 → 不再透出（消费端 3 秒窗口语义下
        // 早已无关紧要，窗口只防陈旧行常驻 payload）
        assert_eq!(
            collect_failure_event_ms(&conn, now - 5_000).unwrap(),
            None,
            "窗口外/无失败轮时应返回 None"
        );
        // 异常中断行（completed_at 为 NULL）无法定位失败时刻 → 不透出
        conn.execute_batch(
            "INSERT INTO turn_usage VALUES
               ('s1', 'turn_broken', 'error', 1, NULL, 0, 0);",
        )
        .unwrap();
        assert_eq!(
            collect_failure_event_ms(&conn, 0).unwrap(),
            Some(now - 10_000),
            "NULL 完成时刻的失败行应被 MAX 忽略（不干扰已有值）"
        );
        drop(conn);
        let _ = fs::remove_file(&path);

        // 判定列降级：缺 cancelled_by_user / tool_error_count 列 → 仅按
        // status 判定（成功轮内的取消/工具报错漏判属可接受降级）
        let (conn, path) = temp_db("fe-degrade");
        conn.execute_batch(
            "CREATE TABLE turn_usage (
                session_id TEXT, turn_id TEXT, status TEXT, started_at INTEGER,
                completed_at INTEGER);",
        )
        .unwrap();
        conn.execute_batch(
            "INSERT INTO turn_usage VALUES
               ('s1', 't_err', 'error', 1000, 2000),
               ('s1', 't_ubye', 'completed', 3000, 4000);",
        )
        .unwrap();
        assert_eq!(
            collect_failure_event_ms(&conn, 0).unwrap(),
            Some(2000),
            "缺判定列时应仅按 status 判定（error 命中，completed 漏判）"
        );
        drop(conn);
        let _ = fs::remove_file(&path);

        // completed_at 列缺失 → fe 信号整体缺失（无失败时刻可取）
        let (conn, path) = temp_db("fe-no-completed");
        conn.execute_batch(
            "CREATE TABLE turn_usage (
                session_id TEXT, turn_id TEXT, status TEXT, started_at INTEGER,
                cancelled_by_user INTEGER);",
        )
        .unwrap();
        assert_eq!(
            collect_failure_event_ms(&conn, 0).unwrap(),
            None,
            "缺 completed_at 列应返回 None"
        );
        drop(conn);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn umid导出_列存在取值_缺失或null输出null() {
        // 场景一：user_message_id 列存在且有值 / 值为 null
        let (conn, path) = temp_db("umid-full");
        conn.execute_batch(
            "CREATE TABLE turn_usage (
                session_id TEXT, turn_id TEXT, status TEXT, started_at INTEGER,
                user_message_id TEXT);",
        )
        .unwrap();
        conn.execute_batch(
            "INSERT INTO turn_usage VALUES ('sess_1', 'turn_1', 'completed', 1000, 'msg_abc'),
             ('sess_1', 'turn_2', 'completed', 2000, NULL);",
        )
        .unwrap();
        let (out, _orphans) = collect_turns(&conn, 0).unwrap().expect("应有输出");
        assert_eq!(out.len(), 2);
        assert_eq!(
            out[0].user_message_id.as_deref(),
            Some("msg_abc"),
            "有值应原样导出（DOM 匹配键）"
        );
        assert_eq!(out[1].user_message_id, None, "值为 null 应导出 null");
        // 序列化形态：None → "umid":null（消费端按 null 跳过匹配）
        let json = serde_json::to_string(&out).unwrap();
        assert!(json.contains("\"umid\":\"msg_abc\""), "{json}");
        assert!(json.contains("\"umid\":null"), "{json}");
        drop(conn);
        let _ = fs::remove_file(&path);

        // 场景二：user_message_id 列缺失（老版本库）→ 整列降级 null
        let (conn, path) = temp_db("umid-missing-col");
        conn.execute_batch(
            "CREATE TABLE turn_usage (
                session_id TEXT, turn_id TEXT, status TEXT, started_at INTEGER);",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO turn_usage VALUES ('sess_1', 'turn_1', 'completed', 1000)",
            [],
        )
        .unwrap();
        let (out, _orphans) = collect_turns(&conn, 0).unwrap().expect("应有输出");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].user_message_id, None, "缺列应降级导出 null");
        drop(conn);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn 导出上限_超出3000轮保留最新() {
        let (conn, path) = temp_db("cap");
        conn.execute_batch(
            "CREATE TABLE turn_usage (
                session_id TEXT, turn_id TEXT, status TEXT, started_at INTEGER);",
        )
        .unwrap();
        // 插入 3005 轮（started_at 递增），应仅保留最新 3000 轮
        let mut sql = String::from(
            "INSERT INTO turn_usage (session_id, turn_id, status, started_at) VALUES",
        );
        for i in 0..(MAX_TURNS + 5) {
            if i > 0 {
                sql.push(',');
            }
            sql.push_str(&format!("('sess_1', 'turn_{i}', 'completed', {})", 1000 + i));
        }
        conn.execute_batch(&sql).unwrap();
        let (out, _orphans) = collect_turns(&conn, 0).unwrap().expect("应有输出");
        assert_eq!(out.len(), MAX_TURNS, "应截断到 3000 轮");
        // 序列按 started_at 升序：保留的是最新的 3000 轮（丢弃头部 5 条最旧的）
        assert_eq!(out[0].turn_id, "turn_5", "最旧的 5 轮应被丢弃");
        assert_eq!(
            out.last().unwrap().turn_id,
            format!("turn_{}", MAX_TURNS + 4),
            "最新一轮应保留在末尾"
        );
        drop(conn);
        let _ = fs::remove_file(&path);
    }

    /// runs 测试用完整库（session + turn_usage + model_usage）
    fn runs_db(name: &str) -> (Connection, std::path::PathBuf) {
        let (conn, path) = temp_db(name);
        conn.execute_batch(
            "CREATE TABLE session (id TEXT PRIMARY KEY, parent_id TEXT);
             CREATE TABLE turn_usage (
                session_id TEXT, turn_id TEXT, status TEXT, started_at INTEGER,
                user_message_id TEXT);
             CREATE TABLE model_usage (
                session_id TEXT, turn_id TEXT, started_at INTEGER,
                parent_user_message_id TEXT,
                input_tokens INTEGER, output_tokens INTEGER, reasoning_tokens INTEGER,
                cache_creation_input_tokens INTEGER, cache_read_input_tokens INTEGER);",
        )
        .unwrap();
        (conn, path)
    }

    #[test]
    fn runs聚合_多请求求和与计数_完成轮与陈旧轮排除() {
        let (conn, path) = runs_db("agg");
        // 固定"当前时刻"：now、近 10 分钟窗口起点、7 天扫查窗口起点
        let now = 1_000_000_000_i64;
        let recent = now - RUN_WINDOW_MS;
        let sweep = now - WINDOW_MS;
        conn.execute_batch(&format!(
            "INSERT INTO session VALUES ('sess_main', NULL);
             -- 进行中轮：3 行在近 10 分钟内 + 1 行在 20 分钟前（长轮早期
             -- 请求，不在新鲜度窗口但在扫查窗口内，聚合应完整计入）
             INSERT INTO model_usage (session_id, turn_id, started_at,
                parent_user_message_id, input_tokens, output_tokens,
                reasoning_tokens, cache_creation_input_tokens,
                cache_read_input_tokens) VALUES
               ('sess_main', 'turn_live', {m1}, 'msg_live', 100, 200, 5, 10, 50),
               ('sess_main', 'turn_live', {m2}, 'msg_live', 110, 21, 0, 0, 60),
               ('sess_main', 'turn_live', {m3}, 'msg_live', 120, 22, 7, 2, 70),
               ('sess_main', 'turn_live', {m4}, 'msg_live', 130, 23, 0, 0, 80);
             -- 已完成轮：model_usage 有行但 turn_usage 也有行 → 排除
             INSERT INTO model_usage (session_id, turn_id, started_at,
                parent_user_message_id, input_tokens) VALUES
               ('sess_main', 'turn_done', {d1}, 'msg_done', 999);
             INSERT INTO turn_usage VALUES
               ('sess_main', 'turn_done', 'completed', {d2}, 'msg_done');
             -- 陈旧轮：请求全部早于新鲜度窗口 → 排除
             INSERT INTO model_usage (session_id, turn_id, started_at,
                parent_user_message_id, input_tokens) VALUES
               ('sess_main', 'turn_old', {o1}, 'msg_old', 888);",
            m1 = now - 1_000,
            m2 = now - 2_000,
            m3 = now - 3_000,
            m4 = now - 1_200_000, /* 20 分钟前：新鲜度窗口外 */
            d1 = now - 5_000,
            d2 = now - 4_000,
            o1 = now - 1_200_000,
        ))
        .unwrap();
        let done = collect_done_turn_ids(&conn, sweep).unwrap();
        assert!(done.contains("turn_done"), "完成轮应进入 done 集合");
        let runs = collect_runs(&conn, recent, sweep, &done, &[]).unwrap();
        assert_eq!(runs.len(), 1, "仅 turn_live 应出现在 runs：{runs:?}");
        let r = &runs[0];
        assert_eq!(r.session_id, "sess_main");
        assert_eq!(r.user_message_id.as_deref(), Some("msg_live"));
        assert_eq!(r.parent_session_id, None, "主会话 psess 应为 null");
        // 4 行完整聚合（含新鲜度窗口外的长轮早期请求）
        assert_eq!(r.input_tokens, 100 + 110 + 120 + 130);
        assert_eq!(r.output_tokens, 200 + 21 + 22 + 23);
        assert_eq!(r.cache_read, 50 + 60 + 70 + 80);
        assert_eq!(r.cache_write, 10 + 2);
        assert_eq!(r.reasoning, 5 + 7);
        assert_eq!(r.requests, 4, "req 应为 model_usage 行数");
        assert_eq!(r.start, now - 1_200_000, "start 应为最早请求时刻");
        drop(conn);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn runs_子代理psess并入父会话_窗口过滤() {
        let (conn, path) = runs_db("psess");
        let now = 2_000_000_000_i64;
        let recent = now - RUN_WINDOW_MS;
        let sweep = now - WINDOW_MS;
        conn.execute_batch(&format!(
            "INSERT INTO session VALUES
               ('sess_main', NULL), ('sess_subagent_agent_1', 'sess_main');
             INSERT INTO model_usage (session_id, turn_id, started_at,
                parent_user_message_id, input_tokens) VALUES
               -- 子代理进行中轮：umid 指向子代理自己的消息（主会话 DOM
               -- 匹配不到），psess 指回父会话
               ('sess_subagent_agent_1', 'turn_s', {t1}, 'msg_child', 30),
               -- 主会话进行中轮
               ('sess_main', 'turn_m', {t2}, 'msg_main', 100);",
            t1 = now - 4_000,
            t2 = now - 2_000,
        ))
        .unwrap();
        let done = collect_done_turn_ids(&conn, sweep).unwrap();
        let runs = collect_runs(&conn, recent, sweep, &done, &[]).unwrap();
        assert_eq!(runs.len(), 2, "{runs:?}");
        let sub = runs.iter().find(|r| r.session_id == "sess_subagent_agent_1");
        let main = runs.iter().find(|r| r.session_id == "sess_main");
        assert_eq!(
            sub.map(|r| r.parent_session_id.as_deref()),
            Some(Some("sess_main")),
            "子代理 run 的 psess 应指回父会话"
        );
        assert_eq!(
            sub.map(|r| r.user_message_id.as_deref()),
            Some(Some("msg_child"))
        );
        assert_eq!(
            main.map(|r| r.parent_session_id.as_deref()),
            Some(None),
            "主会话 run 的 psess 应为 null"
        );
        // V9：父会话存在进行中主轮行 → 子代理行打 m:1（其值已并入主轮行
        // sub）；主会话行不带 m、携带 sub 聚合（n=1，子代理行 30 in）
        assert_eq!(sub.map(|r| r.merged), Some(Some(1)), "子代理行应带 m:1：{runs:?}");
        assert_eq!(main.map(|r| r.merged), Some(None), "主会话行不打 m：{runs:?}");
        let main_sub = main.and_then(|r| r.sub.as_ref()).expect("主会话行应有 sub");
        assert_eq!(main_sub.n, 1);
        assert_eq!(main_sub.input_tokens, 30);
        drop(conn);
        let _ = fs::remove_file(&path);
    }

    /// V9 端到端库：完整 schema（含 completed_at / 统计列），供
    /// collect_turns → collect_runs 全管线断言
    fn v9_db(name: &str) -> (Connection, std::path::PathBuf) {
        let (conn, path) = temp_db(name);
        conn.execute_batch(
            "CREATE TABLE session (id TEXT PRIMARY KEY, parent_id TEXT);
             CREATE TABLE turn_usage (
                session_id TEXT, turn_id TEXT, status TEXT, started_at INTEGER,
                completed_at INTEGER, user_message_id TEXT,
                input_tokens INTEGER, output_tokens INTEGER, reasoning_tokens INTEGER,
                cache_creation_input_tokens INTEGER, cache_read_input_tokens INTEGER,
                model_request_count INTEGER, model_retry_count INTEGER, tool_call_count INTEGER);
             CREATE TABLE model_usage (
                session_id TEXT, turn_id TEXT, started_at INTEGER,
                parent_user_message_id TEXT, model_id TEXT,
                input_tokens INTEGER, output_tokens INTEGER, reasoning_tokens INTEGER,
                cache_creation_input_tokens INTEGER, cache_read_input_tokens INTEGER);",
        )
        .unwrap();
        (conn, path)
    }

    #[test]
    fn runs_主轮sub聚合_并行子代理与游离子轮_m防双计() {
        let (conn, path) = v9_db("v9-agg");
        let now = 5_000_000_000_i64;
        let recent = now - RUN_WINDOW_MS;
        let sweep = now - WINDOW_MS;
        conn.execute_batch(&format!(
            "INSERT INTO session VALUES
               ('sess_main', NULL), ('sess_subagent_a', 'sess_main'),
               ('sess_subagent_b', 'sess_main'), ('sess_subagent_c', 'sess_main');
             -- 主会话进行中轮（首笔请求已完成，主轮 runs 行存在）
             INSERT INTO model_usage (session_id, turn_id, started_at,
                parent_user_message_id, input_tokens, model_id) VALUES
               ('sess_main', 'turn_m', {m1}, 'msg_main', 100, 'GLM-5.3');
             -- 并行两个子代理进行中轮（多子代理并行的实时行）
             INSERT INTO model_usage (session_id, turn_id, started_at,
                parent_user_message_id, input_tokens, model_id) VALUES
               ('sess_subagent_a', 'turn_sa', {m2}, 'msg_sub_a', 30, 'GLM-4.7'),
               ('sess_subagent_a', 'turn_sa', {m2}, 'msg_sub_a', 5, 'GLM-4.7'),
               ('sess_subagent_b', 'turn_sb', {m3}, 'msg_sub_b', 40, 'GLM-4.7');
             -- 游离子代理完成轮：子代理 turn_usage 已落库，但所属主轮
             -- turn_m 尚未落库 → 不进 turns，分流给 runs 侧聚合
             INSERT INTO model_usage (session_id, turn_id, started_at,
                parent_user_message_id, input_tokens, model_id) VALUES
               ('sess_subagent_a', 'turn_s_orphan', {o1}, 'msg_sub_a', 50, 'GLM-4.7');
             INSERT INTO turn_usage VALUES
               ('sess_subagent_a', 'turn_s_orphan', 'completed', {o1}, {o2},
                'msg_sub_a', 50, 60, 0, 0, 20, 3, 0, 0);
             -- 已完成主轮及其子轮：整轮并入 turns（不得进 runs.sub 防双计）
             INSERT INTO turn_usage VALUES
               ('sess_main', 'turn_m_done', 'completed', {d1}, {d2},
                'msg_done', 100, 200, 0, 0, 50, 2, 0, 1),
               ('sess_subagent_c', 'turn_s_merged', 'completed', {d3}, {d4},
                'msg_sub_c', 70, 80, 0, 0, 30, 5, 0, 0);",
            m1 = now - 1_000,
            m2 = now - 2_000,
            m3 = now - 3_000,
            o1 = now - 4_000,
            o2 = now - 3_500,
            d1 = now - 60_000,
            d2 = now - 50_000,
            d3 = now - 58_000,
            d4 = now - 52_000,
        ))
        .unwrap();
        let (turns, orphans) = collect_turns(&conn, sweep).unwrap().expect("应有输出");
        // 完成侧：主会话完成轮（其子轮已整轮并入，sub.n=1, in=70）+
        // 两条子代理自身视图行（V20：turn_s_orphan 与 turn_s_merged 均
        // 已落 turn_usage，无论并入/孤儿/越界取舍都导出自身行）
        assert_eq!(turns.len(), 3, "{turns:?}");
        let done_turn = turns.iter().find(|t| t.turn_id == "turn_m_done").unwrap();
        let done_sub = done_turn.sub.as_ref().expect("完成轮应有子聚合");
        assert_eq!(done_sub.n, 1);
        assert_eq!(done_sub.input_tokens, 70);
        assert_eq!(done_sub.requests, 5);
        let orphan_self = turns
            .iter()
            .find(|t| t.turn_id == "turn_s_orphan")
            .expect("孤儿子轮也应有自身视图行");
        assert_eq!(orphan_self.session_id, "sess_subagent_a");
        assert_eq!(orphan_self.subagent, Some(1));
        assert_eq!(orphan_self.input_tokens, 50, "自身视图行数值为子轮自身值");
        let merged_self = turns
            .iter()
            .find(|t| t.turn_id == "turn_s_merged")
            .expect("已并入主轮的子轮也应有自身视图行");
        assert_eq!(merged_self.input_tokens, 70);
        assert_eq!(merged_self.subagent, Some(1));
        // 游离子代理完成轮：仅未落库主轮的 turn_s_orphan
        assert_eq!(orphans.len(), 1, "{orphans:?}");
        assert_eq!(orphans[0].turn_id, "turn_s_orphan");
        assert_eq!(orphans[0].parent_session_id.as_deref(), Some("sess_main"));
        assert_eq!(orphans[0].input_tokens, 50);
        assert_eq!(orphans[0].requests, 3);

        // runs 侧：主轮行 sub = 并行子代理行 + 游离子代理完成轮
        let done = collect_done_turn_ids(&conn, sweep).unwrap();
        let runs = collect_runs(&conn, recent, sweep, &done, &orphans).unwrap();
        assert_eq!(runs.len(), 3, "仅 3 个进行中轮：{runs:?}");
        let main = runs.iter().find(|r| r.session_id == "sess_main").unwrap();
        assert_eq!(main.merged, None, "主会话行不打 m");
        let sub = main.sub.as_ref().expect("主轮行应有 sub 聚合");
        // n = 2 条子代理实时行 + 1 条游离子代理完成轮
        assert_eq!(sub.n, 3);
        assert_eq!(sub.requests, 2 + 1 + 3);
        // 防双计：恰好 = 子代理实时行（30+5+40）+ 游离子轮（50），
        // 已并入 turns 的 turn_s_merged（in=70）不在其中
        assert_eq!(sub.input_tokens, 125);
        // 子代理实时行：父会话存在主轮行 → 打 m:1（渲染端会话累计跳过）
        for r in runs.iter().filter(|r| r.session_id != "sess_main") {
            assert_eq!(r.merged, Some(1), "子代理行应带 m:1：{r:?}");
            assert_eq!(r.sub, None, "子代理行不携带 sub");
        }
        drop(conn);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn runs_子代理行父会话无主行时不打m标记() {
        // 主轮首笔请求未完成（sess_main 无 model_usage 行）时子代理行不打
        // m:1——渲染端会话条按 psess 直接并入，主轮行出现后自动切换口径
        let (conn, path) = runs_db("v9-no-main");
        let now = 6_000_000_000_i64;
        conn.execute_batch(&format!(
            "INSERT INTO session VALUES
               ('sess_main', NULL), ('sess_subagent_agent_1', 'sess_main');
             INSERT INTO model_usage (session_id, turn_id, started_at,
                parent_user_message_id, input_tokens) VALUES
               ('sess_subagent_agent_1', 'turn_s', {t1}, 'msg_child', 30);",
            t1 = now - 2_000,
        ))
        .unwrap();
        let sweep = now - WINDOW_MS;
        let done = collect_done_turn_ids(&conn, sweep).unwrap();
        let runs = collect_runs(&conn, now - RUN_WINDOW_MS, sweep, &done, &[]).unwrap();
        assert_eq!(runs.len(), 1, "{runs:?}");
        assert_eq!(runs[0].session_id, "sess_subagent_agent_1");
        assert_eq!(
            runs[0].merged, None,
            "父会话无主轮行时不打 m（会话条按 psess 兜底并入）"
        );
        assert_eq!(runs[0].sub, None);
        drop(conn);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn runs_父会话保活_主轮静默超10分钟但子代理活跃时保留() {
        // 修复一：主轮派发子代理后自身静默等待，首笔请求已超出 10 分钟
        // 新鲜度窗口；子代理近 10 分钟在产生请求 → 父会话保活，主轮行
        // 不从 runs 消失（否则前端每轮条跌回全 0 启动窗口占位、会话累计
        // 丢失该轮），子代理行正常打 m:1 并入主轮行 sub
        let (conn, path) = v9_db("v20-keepalive");
        let now = 7_000_000_000_i64;
        let recent = now - RUN_WINDOW_MS;
        let sweep = now - WINDOW_MS;
        conn.execute_batch(&format!(
            "INSERT INTO session VALUES
               ('sess_main', NULL), ('sess_subagent_agent_1', 'sess_main');
             -- 主轮：仅一笔请求在 20 分钟前（超出新鲜度窗口，长轮早期
             -- 请求仍应完整计入组内聚合）
             INSERT INTO model_usage (session_id, turn_id, started_at,
                parent_user_message_id, input_tokens, model_id) VALUES
               ('sess_main', 'turn_m', {m1}, 'msg_main', 100, 'GLM-5.3');
             -- 子代理进行中轮：近 1 分钟有请求 → sess_main 进保活集合
             INSERT INTO model_usage (session_id, turn_id, started_at,
                parent_user_message_id, input_tokens, model_id) VALUES
               ('sess_subagent_agent_1', 'turn_s', {s1}, 'msg_child', 30, 'GLM-4.7');",
            m1 = now - 1_200_000,
            s1 = now - 60_000,
        ))
        .unwrap();
        let done = collect_done_turn_ids(&conn, sweep).unwrap();
        let runs = collect_runs(&conn, recent, sweep, &done, &[]).unwrap();
        assert_eq!(runs.len(), 2, "保活主轮行与子代理行都应在 runs：{runs:?}");
        let main = runs
            .iter()
            .find(|r| r.session_id == "sess_main")
            .expect("主轮行应被父会话保活保留");
        // 组内聚合仍含新鲜度窗口外的早期请求（长轮完整合计）
        assert_eq!(main.input_tokens, 100);
        assert_eq!(main.start, now - 1_200_000);
        assert_eq!(main.merged, None, "主会话行不打 m");
        // 子代理行打 m:1（数值已并入主轮行 sub，渲染端会话累计跳过防双计）
        let sub_run = runs
            .iter()
            .find(|r| r.session_id == "sess_subagent_agent_1")
            .unwrap();
        assert_eq!(sub_run.merged, Some(1), "子代理行应带 m:1：{runs:?}");
        let main_sub = main.sub.as_ref().expect("保活主轮行应有 sub 聚合");
        assert_eq!(main_sub.n, 1);
        assert_eq!(main_sub.input_tokens, 30);
        drop(conn);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn runs_父会话保活_无子代理活动时僵尸主轮仍退出() {
        // 修复一的反向契约：无子代理保活的僵尸主轮（异常中断、turn_usage
        // 永不落库）仍在 10 分钟静默后退出 runs（新鲜度窗口防陈旧语义
        // 不因保活分支放宽）
        let (conn, path) = v9_db("v20-keepalive-off");
        let now = 7_100_000_000_i64;
        let recent = now - RUN_WINDOW_MS;
        let sweep = now - WINDOW_MS;
        conn.execute_batch(&format!(
            "INSERT INTO session VALUES ('sess_main', NULL);
             -- 僵尸主轮：请求全部在 20 分钟前，会话名下无任何子代理活动
             INSERT INTO model_usage (session_id, turn_id, started_at,
                parent_user_message_id, input_tokens, model_id) VALUES
               ('sess_main', 'turn_zombie', {m1}, 'msg_main', 100, 'GLM-5.3');",
            m1 = now - 1_200_000,
        ))
        .unwrap();
        let done = collect_done_turn_ids(&conn, sweep).unwrap();
        let runs = collect_runs(&conn, recent, sweep, &done, &[]).unwrap();
        assert!(
            runs.is_empty(),
            "无子代理保活的僵尸主轮应在 10 分钟静默后退出 runs：{runs:?}"
        );
        drop(conn);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn runs_孤儿并入保活主轮_子代理完成主轮未落库时不悬空() {
        // 修复一 + 根因二：主轮静默超窗（15 分钟前首笔请求），子代理轮
        // 已完成落 turn_usage（其最后一笔 model_usage 请求在窗口内 → 保
        // 活父会话）而主轮尚未落库 → 子轮分流为孤儿，应并入保留的主轮
        // 行 sub 而非悬空蒸发
        let (conn, path) = v9_db("v20-orphan");
        let now = 8_000_000_000_i64;
        let recent = now - RUN_WINDOW_MS;
        let sweep = now - WINDOW_MS;
        conn.execute_batch(&format!(
            "INSERT INTO session VALUES
               ('sess_main', NULL), ('sess_subagent_agent_1', 'sess_main');
             -- 主轮：仅一笔请求在 15 分钟前（超出新鲜度窗口），无
             -- turn_usage 行（派发子代理后静默等待）
             INSERT INTO model_usage (session_id, turn_id, started_at,
                parent_user_message_id, input_tokens, model_id) VALUES
               ('sess_main', 'turn_m', {m1}, 'msg_main', 100, 'GLM-5.3');
             -- 子代理完成轮：最后一笔请求 6 分钟前（窗口内 → 保活），
             -- turn_usage 已落库但主轮 turn_m 未落库
             INSERT INTO model_usage (session_id, turn_id, started_at,
                parent_user_message_id, input_tokens, model_id) VALUES
               ('sess_subagent_agent_1', 'turn_s', {s1}, 'msg_child', 50, 'GLM-4.7');
             INSERT INTO turn_usage VALUES
               ('sess_subagent_agent_1', 'turn_s', 'completed', {s2}, {s3},
                'msg_child', 50, 60, 0, 0, 20, 3, 0, 0);",
            m1 = now - 900_000,
            s1 = now - 360_000,
            s2 = now - 400_000,
            s3 = now - 360_000,
        ))
        .unwrap();
        let (turns, orphans) = collect_turns(&conn, sweep).unwrap().expect("应有输出");
        // 主轮未落库 → turns 仅子代理自身视图行（V20），无主会话行
        assert_eq!(turns.len(), 1, "{turns:?}");
        assert_eq!(turns[0].session_id, "sess_subagent_agent_1");
        assert_eq!(turns[0].subagent, Some(1));
        // 孤儿分流：父会话无覆盖子轮开始时刻的完成主轮（主轮不在 turns）
        assert_eq!(orphans.len(), 1, "{orphans:?}");
        assert_eq!(orphans[0].turn_id, "turn_s");
        // runs：子代理行被 done 集合过滤（turn_usage 已落库），保活主轮
        // 行保留且孤儿并入其 sub（有挂载点，不悬空蒸发）
        let done = collect_done_turn_ids(&conn, sweep).unwrap();
        assert!(done.contains("turn_s"), "已落库子轮应进 done 集合");
        let runs = collect_runs(&conn, recent, sweep, &done, &orphans).unwrap();
        assert_eq!(runs.len(), 1, "{runs:?}");
        assert_eq!(runs[0].session_id, "sess_main", "保活主轮行应保留");
        let sub = runs[0].sub.as_ref().expect("孤儿应并入保活主轮行 sub");
        assert_eq!(sub.n, 1);
        assert_eq!(sub.input_tokens, 50, "孤儿数值取子轮 turn_usage 自身值");
        assert_eq!(sub.requests, 3);
        drop(conn);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn turns_子代理自身视图行_并存导出且主会话聚合不双计() {
        // 修复二：子代理完成轮除并入主轮外，额外导出"自身视图行"
        // （sess/umid 为子代理自己的、数值/dur/ttft 为子轮自身口径、带
        // subagent:1 标记），主轮行仍照常含并入的 sub 数值；主会话 turns
        // 聚合（前端 sessionTotals 按 t.sess 精确匹配）不因自身行双计
        let (conn, path) = v9_db("v20-selfview");
        let now = 9_000_000_000_i64;
        let sweep = now - WINDOW_MS;
        conn.execute_batch(&format!(
            "INSERT INTO session VALUES
               ('sess_main', NULL), ('sess_subagent_agent_1', 'sess_main');
             INSERT INTO model_usage (session_id, turn_id, started_at,
                parent_user_message_id, input_tokens, model_id) VALUES
               ('sess_main', 'turn_m', {m1}, 'msg_main', 100, 'GLM-5.3'),
               ('sess_subagent_agent_1', 'turn_s', {s1}, 'msg_child', 30, 'GLM-4.7');
             INSERT INTO turn_usage VALUES
               ('sess_main', 'turn_m', 'completed', {m1}, {m2},
                'msg_main', 100, 200, 0, 0, 50, 2, 0, 1),
               ('sess_subagent_agent_1', 'turn_s', 'completed', {s1}, {s2},
                'msg_child', 30, 40, 0, 0, 20, 1, 0, 0);
             -- 补齐 dur/ttft 列并写入子轮自身口径值（自身行透出校验）
             ALTER TABLE turn_usage ADD COLUMN duration_ms INTEGER;
             ALTER TABLE turn_usage ADD COLUMN time_to_first_token_ms INTEGER;
             UPDATE turn_usage SET duration_ms = 4000, time_to_first_token_ms = 900
               WHERE turn_id = 'turn_m';
             UPDATE turn_usage SET duration_ms = 350, time_to_first_token_ms = 100
               WHERE turn_id = 'turn_s';",
            m1 = now - 60_000,
            m2 = now - 50_000,
            s1 = now - 55_000,
            s2 = now - 52_000,
        ))
        .unwrap();
        let (turns, orphans) = collect_turns(&conn, sweep).unwrap().expect("应有输出");
        assert!(orphans.is_empty());
        // 两行并存：主轮行（含并入 sub）+ 子代理自身视图行（升序排列）
        assert_eq!(turns.len(), 2, "{turns:?}");
        assert_eq!(turns[0].turn_id, "turn_m", "按 started_at 升序排列");
        assert_eq!(turns[1].turn_id, "turn_s");
        let main = turns.iter().find(|t| t.turn_id == "turn_m").unwrap();
        let self_view = turns.iter().find(|t| t.turn_id == "turn_s").unwrap();
        // 主轮行：无 subagent 标记，数值 = 自身 + 并入子轮（100+30），
        // dur/ttft 保持主轮自身口径
        assert_eq!(main.subagent, None);
        assert_eq!(main.input_tokens, 130);
        assert_eq!(main.sub.as_ref().unwrap().n, 1);
        assert_eq!(main.dur, Some(4000));
        assert_eq!(main.ttft, Some(900));
        // 自身视图行：sess/umid 为子代理自己的、数值/dur/ttft 为子轮
        // 自身口径、带 subagent:1 标记、不携带并入聚合
        assert_eq!(self_view.session_id, "sess_subagent_agent_1");
        assert_eq!(self_view.user_message_id.as_deref(), Some("msg_child"));
        assert_eq!(self_view.subagent, Some(1));
        assert_eq!(self_view.input_tokens, 30);
        assert_eq!(self_view.output_tokens, 40);
        assert_eq!(self_view.cache_read, 20);
        assert_eq!(self_view.requests, 1);
        assert_eq!(self_view.sub, None);
        assert_eq!(self_view.dur, Some(350));
        assert_eq!(self_view.ttft, Some(100));
        // 防双计（模拟前端 sessionTotals 的 t.sess 精确匹配聚合）：主会
        // 话视图只计主轮行一次（130，不叠自身行的 30）；子代理视图只计
        // 自身行（30）
        let sum_in = |sess: &str| {
            turns
                .iter()
                .filter(|t| t.session_id == sess)
                .map(|t| t.input_tokens)
                .sum::<i64>()
        };
        assert_eq!(sum_in("sess_main"), 130, "主会话聚合不应双计子代理自身行");
        assert_eq!(sum_in("sess_subagent_agent_1"), 30);
        // 序列化形态：附加 subagent:1 短键（v2 附加字段，旧前端忽略）
        let json = serde_json::to_string(&turns).unwrap();
        assert!(json.contains("\"subagent\":1"), "{json}");
        let main_json = serde_json::to_string(main).unwrap();
        assert!(!main_json.contains("\"subagent\""), "{main_json}");
        drop(conn);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn runs_无表或无行时输出空数组() {
        let now = 3_000_000_000_i64;
        // 场景一：model_usage 表缺失（老版本库）→ 空 runs，不报错
        let (conn, path) = temp_db("runs-no-mu");
        conn.execute_batch(
            "CREATE TABLE turn_usage (session_id TEXT, turn_id TEXT, status TEXT,
                started_at INTEGER);",
        )
        .unwrap();
        let done = collect_done_turn_ids(&conn, now - WINDOW_MS).unwrap();
        let runs =
            collect_runs(&conn, now - RUN_WINDOW_MS, now - WINDOW_MS, &done, &[]).unwrap();
        assert!(runs.is_empty(), "无 model_usage 表应为空 runs");
        drop(conn);
        let _ = fs::remove_file(&path);

        // 场景二：表存在但窗口内无行 → 空 runs
        let (conn, path) = runs_db("runs-empty");
        let done = collect_done_turn_ids(&conn, now - WINDOW_MS).unwrap();
        let runs =
            collect_runs(&conn, now - RUN_WINDOW_MS, now - WINDOW_MS, &done, &[]).unwrap();
        assert!(runs.is_empty(), "无行应为空 runs");
        drop(conn);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn runs_列缺失按0与null降级() {
        // 老版本形态：model_usage 只有 3 个核心列（无统计列、无 umid 列），
        // session 表也缺失 → 数值全 0、umid/psess 双 null、req 正确计数
        let (conn, path) = temp_db("runs-minimal");
        conn.execute_batch(
            "CREATE TABLE turn_usage (session_id TEXT, turn_id TEXT, status TEXT,
                started_at INTEGER);
             CREATE TABLE model_usage (
                session_id TEXT, turn_id TEXT, started_at INTEGER);",
        )
        .unwrap();
        let now = 4_000_000_000_i64;
        conn.execute_batch(&format!(
            "INSERT INTO model_usage VALUES
               ('sess_1', 'turn_1', {t1}), ('sess_1', 'turn_1', {t2});",
            t1 = now - 1_000,
            t2 = now - 2_000,
        ))
        .unwrap();
        let done = collect_done_turn_ids(&conn, now - WINDOW_MS).unwrap();
        let runs =
            collect_runs(&conn, now - RUN_WINDOW_MS, now - WINDOW_MS, &done, &[]).unwrap();
        assert_eq!(runs.len(), 1);
        let r = &runs[0];
        assert_eq!(r.user_message_id, None, "缺 umid 列应降级 null");
        assert_eq!(r.parent_session_id, None, "缺 session 表应降级 null");
        assert_eq!(r.input_tokens, 0, "缺统计列应按 0 降级");
        assert_eq!(r.output_tokens, 0);
        assert_eq!(r.requests, 2, "req 计数不依赖统计列");
        assert_eq!(r.start, now - 2_000);
        assert_eq!(r.merged, None, "主会话行不打 m");
        assert_eq!(r.sub, None, "无子代理时无 sub 聚合");
        drop(conn);
        let _ = fs::remove_file(&path);
    }
}
