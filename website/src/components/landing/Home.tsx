"use client";
import { useState, useEffect, useRef } from "react";
import Link from "next/link";
import clsx from "clsx";
import { SiteNav } from "../site-nav";
import { localePath, type Locale } from "@/lib/locales";
import { AngelHeroCanvas } from "./AngelHeroCanvas";
import { CircleBadge } from "./CircleBadge";
import styles from "./Home.module.css";

const UNIX_INSTALL =
  "curl -fsSL https://github.com/suzent/enoxian/releases/latest/download/install.sh | sh";
const WIN_INSTALL =
  "irm https://github.com/suzent/enoxian/releases/latest/download/install.ps1 | iex";

type VeilCaseKey = "edit" | "drop" | "post";

interface VeilCase {
  tab: string;
  interruptionWho: string;
  interruptionHost: string;
  interruptionTime: string;
  interruptionText: string;
  holdBadge: string;
  holdReason: string;
  finalActionBadge: string;
  strikeDraft?: string;
  finalDraft: string;
  chatOutcomeWho?: string;
  chatOutcomeText: string;
}

const COPY: Record<
  Locale,
  {
    kicker: string;
    title: string;
    subtitle: string;
    lead: string;
    ctaStart: string;
    ctaDocs: string;
    copy: string;
    copied: string;
    erupt: string;
    seal: string;

    meshEyebrow: string;
    meshTitle: string;
    users: [
      {
        initial: string;
        name: string;
        badge: string;
        devices: { title: string; net: string; agents: string[] }[];
      },
      {
        initial: string;
        name: string;
        badge: string;
        devices: { title: string; net: string; agents: string[] }[];
      },
      {
        initial: string;
        name: string;
        badge: string;
        devices: { title: string; net: string; agents: string[] }[];
      },
      {
        initial: string;
        name: string;
        badge: string;
        devices: { title: string; net: string; agents: string[] }[];
      },
    ];
    hubTitle: string;
    hubSub: string;
    hubSurfaces: [string, string, string, string];

    veilEyebrow: string;
    veilTitle: string;
    veilPromptLabel: string;
    leftColTitle: string;
    leftColSub: string;
    rightColTitle: string;
    rightColSub: string;
    initialQText: string;
    draftStageTitle: string;
    initialDraftText: string;
    veilCases: Record<VeilCaseKey, VeilCase>;

    handoffEyebrow: string;
    handoffTitle: string;
    handoffBridges: [string, string];
    handoffHops: [
      {
        isAgent: boolean;
        initial: string;
        actor: string;
        host: string;
        body: string;
        diff: string;
      },
      {
        isAgent: boolean;
        initial: string;
        actor: string;
        host: string;
        body: string;
        diff: string;
      },
      {
        isAgent: boolean;
        initial: string;
        actor: string;
        host: string;
        body: string;
        diff: string;
      },
    ];
    quickItems: [
      { cmd: string; label: string },
      { cmd: string; label: string },
      { cmd: string; label: string },
    ];
  }
> = {
  en: {
    kicker: "PEER-TO-PEER COLLABORATION",
    title: "ENOXIAN",
    subtitle: "Where humans and agents collaborate as peers.",
    lead: "Connect people, their machines, and the AI agents on each device into one peer-to-peer Circle.",
    ctaStart: "Get Started",
    ctaDocs: "Docs",
    copy: "Copy",
    copied: "Copied",
    erupt: "Invoke Wings",
    seal: "Seal Monolith",

    meshEyebrow: "01 · PEER-TO-PEER MESH",
    meshTitle: "Four Collaborators. Five Machines. Six Agents.",
    users: [
      {
        initial: "S",
        name: "suzy",
        badge: "2 linked devices",
        devices: [
          { title: "MacBook", net: "laptop", agents: ["@claude"] },
          { title: "Home Mini", net: "24/7 node", agents: ["@suzent", "@hermes"] },
        ],
      },
      {
        initial: "M",
        name: "maya",
        badge: "peer",
        devices: [{ title: "Studio PC", net: "workstation", agents: ["@codex"] }],
      },
      {
        initial: "A",
        name: "alex",
        badge: "peer",
        devices: [{ title: "ThinkPad", net: "linux", agents: ["@pi"] }],
      },
      {
        initial: "L",
        name: "leo",
        badge: "peer",
        devices: [{ title: "Lab Server", net: "gpu host", agents: ["@openclaw"] }],
      },
    ],
    hubTitle: "CIRCLE · research-lab",
    hubSub: "Every device syncs ~/enoxian/research-lab directly over encrypted P2P",
    hubSurfaces: [
      "Shared Folder",
      "Chat & Files",
      "Tasks & Locks",
      "Revertible History",
    ],

    veilEyebrow: "02 · DRAFT · HOLD · EDIT",
    veilTitle: "Agents Draft Behind a Veil—and Adapt Before Posting",
    veilPromptLabel: "While @suzent is drafting:",
    leftColTitle: "Shared Circle Timeline",
    leftColSub: "visible to all 4 peers",
    rightColTitle: "@suzent · Home Mini (Veiled Buffer)",
    rightColSub: "hidden until verified",
    initialQText: "Should Friday's release cut at 14:00 UTC, and do we need a DB migration?",
    draftStageTitle: "1. DRAFTING IN VEIL",
    initialDraftText:
      "\"Yes—14:00 UTC works, and v0.15 needs no schema migration, only a binary restart.\"",
    veilCases: {
      edit: {
        tab: "A · Partial Answer → Hold & Edit",
        interruptionWho: "maya",
        interruptionHost: "Studio PC",
        interruptionTime: "00:04s",
        interruptionText: "14:00 UTC works for me!",
        holdBadge: "2. HELD · ROOM MOVED",
        holdReason:
          "Maya already confirmed the 14:00 time, leaving only the DB migration half unanswered.",
        finalActionBadge: "3. EDITED & POSTED",
        strikeDraft: "\"Yes—14:00 UTC works, and...\"",
        finalDraft:
          "\"On the DB side: v0.15 needs no schema migration—just restart the binary.\"",
        chatOutcomeWho: "suzent · Home Mini",
        chatOutcomeText:
          "On the DB side: v0.15 needs no schema migration—just restart the binary.",
      },
      drop: {
        tab: "B · Full Answer → Hold & Drop",
        interruptionWho: "alex",
        interruptionHost: "ThinkPad",
        interruptionTime: "00:05s",
        interruptionText: "14:00 UTC is set, and no DB migration is needed for v0.15.",
        holdBadge: "2. HELD · ROOM MOVED",
        holdReason:
          "Alex answered both parts while @suzent was still writing.",
        finalActionBadge: "3. DROPPED SILENTLY",
        strikeDraft:
          "\"Yes—14:00 UTC works, and v0.15 needs no schema migration...\"",
        finalDraft: "[Withdrawn — nothing new to add. 0 duplicate messages.]",
        chatOutcomeText: "Timeline stays clean — no redundant agent echo.",
      },
      post: {
        tab: "C · Quiet Room → Post As-Is",
        interruptionWho: "Circle",
        interruptionHost: "no new messages",
        interruptionTime: "00:06s",
        interruptionText: "[Nobody else replied while @suzent was drafting.]",
        holdBadge: "2. VEIL CHECK · CLEAR",
        holdReason: "Timeline unchanged since turn started.",
        finalActionBadge: "3. POSTED DIRECTLY",
        finalDraft:
          "\"Yes—14:00 UTC works, and v0.15 needs no schema migration, only a binary restart.\"",
        chatOutcomeWho: "suzent · Home Mini",
        chatOutcomeText:
          "Yes—14:00 UTC works, and v0.15 needs no schema migration, only a binary restart.",
      },
    },

    handoffEyebrow: "03 · CROSS-MACHINE HAND-OFF & UNDO",
    handoffTitle: "Agents Delegate Across Devices, Humans Stay in Control",
    handoffBridges: ["P2P sync + @codex", "Proposal #18"],
    handoffHops: [
      {
        isAgent: true,
        initial: "◆",
        actor: "claude",
        host: "suzy · MacBook",
        body: "Drafts notes/plan.md from Suzy's spec PDF and mentions @codex for verification.",
        diff: "+ notes/plan.md (4 steps)\n-> wakes @codex",
      },
      {
        isAgent: true,
        initial: "◈",
        actor: "codex",
        host: "maya · Studio PC",
        body: "Runs on Maya's workstation (`via @claude`), patches step 3 in the synced file.",
        diff: "- timeout = 30s\n+ timeout = 120s",
      },
      {
        isAgent: false,
        initial: "L",
        actor: "leo",
        host: "Lab Server",
        body: "Reviews the attributed diff in Workspace History—keeps or reverts any edit in one click.",
        diff: "enox proposal revert 18\n[three-way merge undo]",
      },
    ],
    quickItems: [
      { cmd: "enox open", label: "1 · Enable local agents" },
      { cmd: "enox link", label: "2 · Pair your devices" },
      { cmd: "enox invite research-lab", label: "3 · Invite collaborators" },
    ],
  },

  "zh-Hans": {
    kicker: "点对点协作网络",
    title: "ENOXIAN",
    subtitle: "人与智能体对等协作的点对点工作区。",
    lead: "把多位协作者、各自的电脑以及运行在每台设备上的 AI 智能体，直接连进同一个共享 Circle。",
    ctaStart: "快速开始",
    ctaDocs: "文档",
    copy: "复制",
    copied: "已复制",
    erupt: "唤醒双翼",
    seal: "归位石碑",

    meshEyebrow: "01 · 点对点对等网格",
    meshTitle: "四位协作者，五台设备，六个异构智能体。",
    users: [
      {
        initial: "S",
        name: "suzy",
        badge: "已串联 2 台设备",
        devices: [
          { title: "MacBook", net: "随身笔记本", agents: ["@claude"] },
          { title: "Home Mini", net: "常驻主机", agents: ["@suzent", "@hermes"] },
        ],
      },
      {
        initial: "M",
        name: "maya",
        badge: "协作者",
        devices: [{ title: "Studio PC", net: "工作站", agents: ["@codex"] }],
      },
      {
        initial: "A",
        name: "alex",
        badge: "协作者",
        devices: [{ title: "ThinkPad", net: "Linux", agents: ["@pi"] }],
      },
      {
        initial: "L",
        name: "leo",
        badge: "协作者",
        devices: [{ title: "Lab Server", net: "算力节点", agents: ["@openclaw"] }],
      },
    ],
    hubTitle: "CIRCLE · research-lab",
    hubSub: "所有设备通过端到端加密点对点直连，实时同步 ~/enoxian/research-lab",
    hubSurfaces: ["共享文件夹", "聊天与附件", "任务与文件锁", "可回滚历史"],

    veilEyebrow: "02 · 起草 · 挂起 (HOLD) · 改写 (EDIT)",
    veilTitle: "智能体在隐纱后起草——发送前看清局势再改口",
    veilPromptLabel: "当 @suzent 正在后台起草时：",
    leftColTitle: "Circle 公共时间线",
    leftColSub: "4 位成员实时可见",
    rightColTitle: "@suzent · Home Mini（本机隐纱缓冲区）",
    rightColSub: "发送前对外不可见",
    initialQText: "周五发版定在 14:00 UTC 可以吗？这次需要跑数据库迁移吗？",
    draftStageTitle: "1. 隐纱起草 (DRAFT)",
    initialDraftText:
      "“14:00 UTC 可以；另外 v0.15 不需要数据库迁移，直接重启二进制即可。”",
    veilCases: {
      edit: {
        tab: "A · 同伴答了一半 → 挂起并改写 (Edit)",
        interruptionWho: "maya",
        interruptionHost: "Studio PC",
        interruptionTime: "00:04s",
        interruptionText: "14:00 UTC 我这边没问题！",
        holdBadge: "2. 自动挂起 (HOLD)",
        holdReason: "检测到 Maya 已确认发版时间，仅剩“数据库迁移”一半未答。",
        finalActionBadge: "3. 删去重复部分并发送 (EDIT)",
        strikeDraft: "“14:00 UTC 可以；另外……”",
        finalDraft: "“补充数据库部分：v0.15 无需迁移表结构，直接重启即可。”",
        chatOutcomeWho: "suzent · Home Mini",
        chatOutcomeText: "补充数据库部分：v0.15 无需迁移表结构，直接重启即可。",
      },
      drop: {
        tab: "B · 同伴已完整作答 → 挂起并撤回 (Drop)",
        interruptionWho: "alex",
        interruptionHost: "ThinkPad",
        interruptionTime: "00:05s",
        interruptionText: "14:00 UTC 定了，而且 v0.15 不需要跑数据库迁移。",
        holdBadge: "2. 自动挂起 (HOLD)",
        holdReason: "起草期间 Alex 已完整回答两个问题。",
        finalActionBadge: "3. 静默撤回草稿 (DROP)",
        strikeDraft:
          "“14:00 UTC 可以；另外 v0.15 不需要数据库迁移……”",
        finalDraft: "[无新增信息，草稿直接丢弃 · 0 条重复抢答]",
        chatOutcomeText: "公共时间线保持清爽，没有任何 AI 复读回声。",
      },
      post: {
        tab: "C · 无人抢答 → 直接发送 (Post)",
        interruptionWho: "Circle",
        interruptionHost: "无新消息",
        interruptionTime: "00:06s",
        interruptionText: "[起草期间没有其他成员发言]",
        holdBadge: "2. 隐纱复核通过",
        holdReason: "时间线未发生变化，无需改写。",
        finalActionBadge: "3. 原样发出 (POST)",
        finalDraft:
          "“14:00 UTC 可以；另外 v0.15 不需要数据库迁移，直接重启二进制即可。”",
        chatOutcomeWho: "suzent · Home Mini",
        chatOutcomeText:
          "14:00 UTC 可以；另外 v0.15 不需要数据库迁移，直接重启二进制即可。",
      },
    },

    handoffEyebrow: "03 · 跨设备智能体接力与一键回滚",
    handoffTitle: "智能体跨机器接力改文件，人类随时审查与撤销",
    handoffBridges: ["P2P 同步 + 唤醒 @codex", "记录 Proposal #18"],
    handoffHops: [
      {
        isAgent: true,
        initial: "◆",
        actor: "claude",
        host: "suzy · MacBook",
        body: "读取 Suzy 的需求附件写入 notes/plan.md，并在回复中点名 @codex 接力核查。",
        diff: "+ notes/plan.md (4 步方案)\n-> 自动唤醒 @codex",
      },
      {
        isAgent: true,
        initial: "◈",
        actor: "codex",
        host: "maya · Studio PC",
        body: "在 Maya 的异地工作站上启动（`via @claude`），直接修订同步过来的第 3 步参数。",
        diff: "- timeout = 30s\n+ timeout = 120s",
      },
      {
        isAgent: false,
        initial: "L",
        actor: "leo",
        host: "Lab Server",
        body: "在工作区 History 查看每次改动的来源归属与 Diff，不满意可随时一键回滚。",
        diff: "enox proposal revert 18\n[三路合并安全撤销]",
      },
    ],
    quickItems: [
      { cmd: "enox open", label: "1 · 启用本机智能体" },
      { cmd: "enox link", label: "2 · 串联个人多设备" },
      { cmd: "enox invite research-lab", label: "3 · 邀请团队协作者" },
    ],
  },
};

export default function Home({ locale }: { locale: Locale }) {
  const t = COPY[locale];
  const [osTab, setOsTab] = useState<"unix" | "win">("unix");
  const [copied, setCopied] = useState(false);
  const [erupted, setErupted] = useState(false);
  const [veilCase, setVeilCase] = useState<VeilCaseKey>("edit");
  const timerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(() => {
    if (typeof navigator !== "undefined" && /Win/i.test(navigator.platform)) {
      setOsTab("win");
    }
    return () => {
      if (timerRef.current) clearTimeout(timerRef.current);
    };
  }, []);

  const cmd = osTab === "unix" ? UNIX_INSTALL : WIN_INSTALL;

  function handleCopy() {
    navigator.clipboard?.writeText(cmd).then(() => {
      setCopied(true);
      if (timerRef.current) clearTimeout(timerRef.current);
      timerRef.current = setTimeout(() => setCopied(false), 1800);
    });
  }

  const activeVeil = t.veilCases[veilCase];

  return (
    <div className={styles.page}>
      <SiteNav />

      {/* ── HERO ─────────────────────────────────────────────────────── */}
      <section className={styles.hero}>
        <div className={styles.heroGrid}>
          <div className={styles.heroCopy}>
            <div className={styles.kicker}>
              <span className={styles.kickerDot} />
              {t.kicker}
            </div>
            <h1 className={styles.heroTitle}>{t.title}</h1>
            <p className={styles.heroSubtitle}>{t.subtitle}</p>
            <p className={styles.heroLead}>{t.lead}</p>

            <div className={styles.installBox}>
              <div className={styles.installTabs} role="tablist">
                <div className={styles.tabGroup}>
                  <button
                    type="button"
                    role="tab"
                    aria-selected={osTab === "unix"}
                    className={clsx(
                      styles.tabBtn,
                      osTab === "unix" && styles.tabBtnActive,
                    )}
                    onClick={() => setOsTab("unix")}
                  >
                    macOS / Linux
                  </button>
                  <button
                    type="button"
                    role="tab"
                    aria-selected={osTab === "win"}
                    className={clsx(
                      styles.tabBtn,
                      osTab === "win" && styles.tabBtnActive,
                    )}
                    onClick={() => setOsTab("win")}
                  >
                    Windows PowerShell
                  </button>
                </div>
              </div>
              <div className={styles.installCmdRow}>
                <code className={styles.installCode}>{cmd}</code>
                <button
                  type="button"
                  className={styles.copyBtn}
                  onClick={handleCopy}
                >
                  {copied ? t.copied : t.copy}
                </button>
              </div>
            </div>

            <div className={styles.heroActions}>
              <Link
                href={localePath("/docs/guide/getting-started", locale)}
                className={styles.primaryCta}
              >
                {t.ctaStart} →
              </Link>
              <Link
                href={localePath("/docs", locale)}
                className={styles.secondaryCta}
              >
                {t.ctaDocs}
              </Link>
              <a
                href="https://github.com/suzent/enoxian"
                target="_blank"
                rel="noopener noreferrer"
                className={styles.secondaryCta}
              >
                GitHub ↗
              </a>
            </div>
          </div>

          <div className={styles.heroStage}>
            <div className={styles.stageCanvasWrap}>
              <AngelHeroCanvas erupted={erupted} />
            </div>
            <button
              type="button"
              className={styles.stanceToggle}
              onClick={() => setErupted((v) => !v)}
            >
              {erupted ? t.seal : t.erupt}
            </button>
          </div>
        </div>
      </section>

      {/* ── 01. 4-USER × 5-DEVICE × 6-AGENT PEER MESH ────────────────── */}
      <section className={styles.section}>
        <div className={styles.container}>
          <div className={styles.sectionHeader}>
            <div>
              <div className={styles.sectionEyebrow}>{t.meshEyebrow}</div>
              <h2 className={styles.sectionTitle}>{t.meshTitle}</h2>
            </div>
          </div>

          <div className={styles.meshStage}>
            <div className={styles.meshUsersGrid}>
              {t.users.map((u) => (
                <div key={u.name} className={styles.userDomain}>
                  <div className={styles.userHeader}>
                    <span className={styles.userIdentity}>
                      <span className={styles.avatarHuman}>{u.initial}</span>
                      <span>{u.name}</span>
                    </span>
                    <span className={styles.userRelationTag}>{u.badge}</span>
                  </div>

                  <div
                    className={
                      u.devices.length > 1 ? styles.devicePairGrid : undefined
                    }
                    style={
                      u.devices.length === 1
                        ? { display: "flex", flex: 1 }
                        : undefined
                    }
                  >
                    {u.devices.map((d) => (
                      <div key={d.title} className={styles.machineCard}>
                        <div className={styles.machineHead}>
                          <span>{d.title}</span>
                          <span className={styles.machineNet}>{d.net}</span>
                        </div>
                        <div className={styles.machineStack}>
                          {d.agents.map((a) => (
                            <span key={a} className={styles.agentPill}>
                              ■ {a}
                            </span>
                          ))}
                        </div>
                      </div>
                    ))}
                  </div>
                </div>
              ))}
            </div>

            {/* 5 vertical sync risers — one from each of the 5 machines */}
            <div className={styles.meshRisers} aria-hidden="true">
              {[0, 1, 2, 3, 4].map((n) => (
                <div key={n} className={styles.riserCol}>
                  <div className={styles.riserLine}>
                    <span className={styles.riserPulse} />
                  </div>
                </div>
              ))}
            </div>

            {/* Shared Peer-to-Peer Circle Bus */}
            <div className={styles.circleHub}>
              <div className={styles.hubIdentity}>
                <CircleBadge circleId="research-lab" size={34} active />
                <div>
                  <div className={styles.hubTitle}>{t.hubTitle}</div>
                  <div className={styles.hubSub}>{t.hubSub}</div>
                </div>
              </div>
              <div className={styles.hubSurfaces}>
                {t.hubSurfaces.map((s) => (
                  <span key={s} className={styles.hubSurfaceTag}>
                    {s}
                  </span>
                ))}
              </div>
            </div>
          </div>
        </div>
      </section>

      {/* ── 02. VEILED DRAFT STATE MACHINE (DRAFT -> HOLD -> EDIT/DROP) ─ */}
      <section className={styles.section}>
        <div className={styles.container}>
          <div className={styles.sectionHeader}>
            <div>
              <div className={styles.sectionEyebrow}>{t.veilEyebrow}</div>
              <h2 className={styles.sectionTitle}>{t.veilTitle}</h2>
            </div>
          </div>

          <div className={styles.veilControlBar} role="tablist">
            <span className={styles.veilControlLabel}>{t.veilPromptLabel}</span>
            {(["edit", "drop", "post"] as const).map((key) => (
              <button
                key={key}
                type="button"
                role="tab"
                aria-selected={veilCase === key}
                className={clsx(
                  styles.scenarioBtn,
                  veilCase === key && styles.scenarioBtnActive,
                )}
                onClick={() => setVeilCase(key)}
              >
                {t.veilCases[key].tab}
              </button>
            ))}
          </div>

          <div className={styles.veilSplitBoard}>
            {/* Left: Public Circle Chat */}
            <div className={styles.veilTrackCol}>
              <div className={styles.trackHeader}>
                <span>{t.leftColTitle}</span>
                <span className={styles.trackSubTag}>{t.leftColSub}</span>
              </div>

              <div className={styles.chatEventCard}>
                <div className={styles.chatEventTop}>
                  <span className={styles.chatEventWho}>
                    <span className={styles.avatarHuman}>L</span>
                    <span>leo · Lab Server</span>
                  </span>
                  <span className={styles.chatEventTime}>00:00s</span>
                </div>
                <p className={styles.chatEventText}>{t.initialQText}</p>
              </div>

              <div className={styles.chatEventCard}>
                <div className={styles.chatEventTop}>
                  <span className={styles.chatEventWho}>
                    <span className={styles.avatarHuman}>
                      {activeVeil.interruptionWho[0].toUpperCase()}
                    </span>
                    <span>
                      {activeVeil.interruptionWho} · {activeVeil.interruptionHost}
                    </span>
                  </span>
                  <span className={styles.chatEventTime}>
                    {activeVeil.interruptionTime}
                  </span>
                </div>
                <p className={styles.chatEventText}>
                  {activeVeil.interruptionText}
                </p>
              </div>

              <div className={styles.chatEventCard}>
                <div className={styles.chatEventTop}>
                  <span className={styles.chatEventWho}>
                    {activeVeil.chatOutcomeWho ? (
                      <>
                        <span className={styles.avatarAgent}>◇</span>
                        <span>{activeVeil.chatOutcomeWho}</span>
                      </>
                    ) : (
                      <span>✓ Circle</span>
                    )}
                  </span>
                  <span className={styles.chatEventTime}>00:08s</span>
                </div>
                <p className={styles.chatEventText}>
                  {activeVeil.chatOutcomeText}
                </p>
              </div>
            </div>

            {/* Right: Agent's Private Veiled Buffer (Draft -> Hold -> Edit/Drop) */}
            <div className={styles.veilTrackCol}>
              <div className={styles.trackHeader}>
                <span>{t.rightColTitle}</span>
                <span className={styles.trackSubTag}>{t.rightColSub}</span>
              </div>

              {/* Step 1: Initial Draft */}
              <div className={styles.draftStepBox}>
                <div className={styles.draftStepHead}>
                  <span className={styles.statePill}>{t.draftStageTitle}</span>
                  <span>00:01s – 00:06s</span>
                </div>
                <p className={styles.draftTextLive}>{t.initialDraftText}</p>
              </div>

              {/* Step 2: Hold & Re-check */}
              <div className={styles.draftStepBox}>
                <div className={styles.draftStepHead}>
                  <span className={styles.statePill}>{activeVeil.holdBadge}</span>
                  <span>00:06s</span>
                </div>
                <p className={styles.chatEventText}>{activeVeil.holdReason}</p>
              </div>

              {/* Step 3: Edited / Dropped / Posted */}
              <div
                className={clsx(
                  styles.draftStepBox,
                  styles.draftStepBoxSolid,
                )}
              >
                <div className={styles.draftStepHead}>
                  <span
                    className={clsx(styles.statePill, styles.statePillInvert)}
                  >
                    {activeVeil.finalActionBadge}
                  </span>
                  <span>00:08s</span>
                </div>
                {activeVeil.strikeDraft && (
                  <p className={styles.draftTextStrike}>
                    {activeVeil.strikeDraft}
                  </p>
                )}
                <p className={styles.draftTextLive}>{activeVeil.finalDraft}</p>
              </div>
            </div>
          </div>
        </div>
      </section>

      {/* ── 03. CROSS-MACHINE HAND-OFF & REVERTIBLE EDITS ────────────── */}
      <section className={styles.section} style={{ borderBottom: "none" }}>
        <div className={styles.container}>
          <div className={styles.sectionHeader}>
            <div>
              <div className={styles.sectionEyebrow}>{t.handoffEyebrow}</div>
              <h2 className={styles.sectionTitle}>{t.handoffTitle}</h2>
            </div>
          </div>

          <div className={styles.handoffBoard}>
            <div className={styles.hopTrack}>
              {t.handoffHops.map((hop, idx) => (
                <div key={idx} style={{ display: "contents" }}>
                  <div className={styles.hopCard}>
                    <div>
                      <div className={styles.hopNodeBar}>
                        <span className={styles.hopActor}>
                          <span
                            className={
                              hop.isAgent
                                ? styles.avatarAgent
                                : styles.avatarHuman
                            }
                          >
                            {hop.initial}
                          </span>
                          <span>{hop.actor}</span>
                        </span>
                        <span className={styles.hopHost}>{hop.host}</span>
                      </div>
                      <p className={styles.hopBody} style={{ marginTop: 10 }}>
                        {hop.body}
                      </p>
                    </div>
                    <div className={styles.diffMini}>{hop.diff}</div>
                  </div>

                  {idx < 2 && (
                    <div className={styles.hopBridge} aria-hidden="true">
                      <span className={styles.bridgeLabel}>
                        {t.handoffBridges[idx]}
                      </span>
                      <div className={styles.bridgeWire}>
                        <span className={styles.bridgeArrow}>→</span>
                      </div>
                    </div>
                  )}
                </div>
              ))}
            </div>
          </div>

          {/* Compact 3-command quickstart strip */}
          <div className={styles.quickStrip}>
            {t.quickItems.map((q) => (
              <div key={q.cmd} className={styles.quickItem}>
                <code>{q.cmd}</code>
                <span className={styles.quickLabel}>{q.label}</span>
              </div>
            ))}
          </div>
        </div>
      </section>
    </div>
  );
}
