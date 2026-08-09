/**
 * Web Speech 连续听写会话。
 *
 * 规则（用户无感知续听）：
 * 1. armed=true 期间，会话结束（句末停顿 / no-speech）必须自动再启，不弹 tip。
 * 2. 重启前把当前已显示文本固化为 base，避免 interim 被下一轮冲掉。
 * 3. 优先复用同一 Recognition；start 失败则换新实例。
 * 4. 看门狗：armed 但未在跑且无排队重启 → 强制拉起。
 * 5. 主动轮换：长连跑约 40s 换新实例，规避 Chrome 静默假死。
 * 6. 仅 not-allowed / service-not-allowed / audio-capture 视为致命并回调；其余软错误静默重试。
 */

type SpeechRec = {
  continuous: boolean;
  interimResults: boolean;
  lang: string;
  maxAlternatives: number;
  start: () => void;
  stop: () => void;
  abort: () => void;
  onstart: (() => void) | null;
  onresult: ((ev: SpeechRecEvent) => void) | null;
  onerror: ((ev: { error: string }) => void) | null;
  onend: (() => void) | null;
};

type SpeechRecEvent = {
  resultIndex: number;
  results: ArrayLike<{
    isFinal: boolean;
    0: { transcript: string };
  }>;
};

export type SpeechDictationHandlers = {
  onText: (text: string) => void;
  onListeningChange: (listening: boolean) => void;
  onFatalError: (code: string) => void;
};

const RESUME_DELAY_MS = 80;
const RETRY_DELAY_MS = 280;
const WATCHDOG_MS = 400;
const STALE_MS = 1800;
const RECYCLE_MS = 40_000;
const FATAL = new Set(["not-allowed", "service-not-allowed", "audio-capture"]);

function getCtor(): (new () => SpeechRec) | null {
  const w = window as Window & {
    SpeechRecognition?: new () => SpeechRec;
    webkitSpeechRecognition?: new () => SpeechRec;
  };
  return w.SpeechRecognition || w.webkitSpeechRecognition || null;
}

export function speechDictationSupported(): boolean {
  return Boolean(getCtor()) && window.isSecureContext;
}

export class SpeechDictation {
  private armed = false;
  private rec: SpeechRec | null = null;
  private running = false;
  private starting = false;
  private restartTimer: number | null = null;
  private watchdogTimer: number | null = null;
  private recycleTimer: number | null = null;
  private base = "";
  private finals = "";
  private lastEmit = "";
  private lastActiveAt = 0;
  private handlers: SpeechDictationHandlers;
  private lang: string;

  constructor(handlers: SpeechDictationHandlers, lang = "zh-CN") {
    this.handlers = handlers;
    this.lang = lang;
  }

  get isArmed(): boolean {
    return this.armed;
  }

  /** 开始听写；seed 为输入框已有文本 */
  start(seed: string) {
    if (!window.isSecureContext || !getCtor()) {
      this.handlers.onFatalError(
        window.isSecureContext ? "unsupported" : "service-not-allowed",
      );
      return;
    }
    this.armed = true;
    this.base = seed;
    this.finals = "";
    this.lastEmit = seed;
    this.lastActiveAt = Date.now();
    this.handlers.onListeningChange(true);
    this.startWatchdog();
    this.bootRecognizer();
  }

  stop() {
    this.armed = false;
    this.clearRestart();
    this.stopWatchdog();
    this.clearRecycle();
    this.killRecognizer();
    this.running = false;
    this.starting = false;
    this.handlers.onListeningChange(false);
  }

  dispose() {
    this.stop();
  }

  private emit(text: string) {
    if (text === this.lastEmit) return;
    this.lastEmit = text;
    this.handlers.onText(text);
  }

  /** 把界面上已有字固化，防止 interim 在重启后丢失 */
  private solidifyDisplayed() {
    this.base = this.lastEmit;
    this.finals = "";
  }

  private clearRestart() {
    if (this.restartTimer != null) {
      window.clearTimeout(this.restartTimer);
      this.restartTimer = null;
    }
  }

  private clearRecycle() {
    if (this.recycleTimer != null) {
      window.clearTimeout(this.recycleTimer);
      this.recycleTimer = null;
    }
  }

  private stopWatchdog() {
    if (this.watchdogTimer != null) {
      window.clearInterval(this.watchdogTimer);
      this.watchdogTimer = null;
    }
  }

  private startWatchdog() {
    this.stopWatchdog();
    this.watchdogTimer = window.setInterval(() => {
      if (!this.armed) return;
      if (this.starting || this.running) {
        // 长连跑主动轮换
        return;
      }
      if (this.restartTimer != null) return;
      if (Date.now() - this.lastActiveAt >= STALE_MS) {
        this.scheduleResume(0, true);
      }
    }, WATCHDOG_MS);
  }

  private armRecycle() {
    this.clearRecycle();
    this.recycleTimer = window.setTimeout(() => {
      this.recycleTimer = null;
      if (!this.armed) return;
      // 主动换新，减少 Chrome 假死
      this.solidifyDisplayed();
      this.bootRecognizer();
    }, RECYCLE_MS);
  }

  private scheduleResume(delayMs: number, forceNew: boolean) {
    if (!this.armed) return;
    this.clearRestart();
    this.restartTimer = window.setTimeout(() => {
      this.restartTimer = null;
      if (!this.armed) return;
      this.solidifyDisplayed();
      if (forceNew || !this.rec) {
        this.bootRecognizer();
        return;
      }
      this.starting = true;
      try {
        this.rec.start();
        this.lastActiveAt = Date.now();
      } catch {
        this.starting = false;
        this.bootRecognizer();
      }
    }, delayMs);
  }

  private killRecognizer() {
    const prev = this.rec;
    this.rec = null;
    if (!prev) return;
    try {
      prev.onstart = null;
      prev.onresult = null;
      prev.onerror = null;
      prev.onend = null;
      prev.abort();
    } catch {
      try {
        prev.stop();
      } catch {
        /* ignore */
      }
    }
  }

  private bootRecognizer() {
    if (!this.armed) return;
    const Ctor = getCtor();
    if (!Ctor) {
      this.armed = false;
      this.handlers.onListeningChange(false);
      this.handlers.onFatalError("unsupported");
      return;
    }

    this.clearRestart();
    this.killRecognizer();
    this.running = false;
    this.starting = true;

    const rec = new Ctor();
    rec.continuous = true;
    rec.interimResults = true;
    rec.maxAlternatives = 1;
    rec.lang = this.lang;

    rec.onstart = () => {
      this.running = true;
      this.starting = false;
      this.lastActiveAt = Date.now();
      this.handlers.onListeningChange(true);
      this.armRecycle();
    };

    rec.onresult = (ev) => {
      this.lastActiveAt = Date.now();
      let interim = "";
      for (let i = ev.resultIndex; i < ev.results.length; i++) {
        const piece = ev.results[i][0]?.transcript || "";
        if (ev.results[i].isFinal) this.finals += piece;
        else interim += piece;
      }
      this.emit(`${this.base}${this.finals}${interim}`);
    };

    rec.onerror = (ev) => {
      const code = ev.error || "unknown";
      this.running = false;
      this.starting = false;
      if (code === "no-speech" || code === "aborted") {
        // 交给 onend 续听
        return;
      }
      if (FATAL.has(code)) {
        this.armed = false;
        this.clearRestart();
        this.stopWatchdog();
        this.clearRecycle();
        this.rec = null;
        this.handlers.onListeningChange(false);
        this.handlers.onFatalError(code);
        return;
      }
      // network 等：静默换新
      if (this.armed) {
        this.scheduleResume(RETRY_DELAY_MS, true);
      }
    };

    rec.onend = () => {
      this.running = false;
      this.starting = false;
      this.clearRecycle();
      if (this.rec === rec) this.rec = null;
      if (!this.armed) {
        this.handlers.onListeningChange(false);
        return;
      }
      this.lastActiveAt = Date.now();
      // 句末停顿：先试同实例语义上的「再启」——实例已 end，直接换新更稳
      this.scheduleResume(RESUME_DELAY_MS, true);
    };

    this.rec = rec;
    try {
      rec.start();
      this.lastActiveAt = Date.now();
    } catch {
      this.starting = false;
      this.scheduleResume(RETRY_DELAY_MS, true);
    }
  }
}
