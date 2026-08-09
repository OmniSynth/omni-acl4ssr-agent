import { Navigate, NavLink, Route, Routes } from "react-router-dom";
import OverviewPage from "./pages/OverviewPage";
import GroupsPage from "./pages/GroupsPage";
import RulesetsPage from "./pages/RulesetsPage";
import LanRoutesPage from "./pages/LanRoutesPage";
import LandingsPage from "./pages/LandingsPage";
import AiPage from "./pages/AiPage";
import { NikkiPanelButton } from "./NikkiPanelButton";
import { NikkiUpdateButton } from "./NikkiUpdateButton";
import { ThemeToggle } from "./theme";

const nav = [
  { to: "/", label: "概况", end: true },
  { to: "/groups", label: "策略组" },
  { to: "/rulesets", label: "规则集" },
  { to: "/lan-routes", label: "局域网分流" },
  { to: "/landings", label: "落地代理" },
];

export default function App() {
  return (
    <div className="flex h-dvh flex-col overflow-hidden bg-app text-fg">
      <header className="z-20 shrink-0 border-b border-edge bg-header backdrop-blur-xl">
        <div className="flex w-full flex-wrap items-center gap-3 px-4 py-2.5">
          <div className="mr-2 min-w-0">
            <div className="text-[15px] font-semibold tracking-tight text-fg">omni-acl4ssr-agent</div>
            <div className="text-[11px] text-fg-soft">本地 Mihomo 订阅转换</div>
          </div>
          <nav className="flex min-w-0 flex-1 flex-wrap items-center gap-0.5">
            {nav.map((item) => (
              <NavLink
                key={item.to}
                to={item.to}
                end={item.end}
                className={({ isActive }) =>
                  [
                    "rounded-lg px-3 py-1.5 text-sm font-medium transition",
                    isActive
                      ? "bg-hover text-nav-active"
                      : "text-fg-soft hover:bg-hover hover:text-fg",
                  ].join(" ")
                }
              >
                {item.label}
              </NavLink>
            ))}
          </nav>
          <div className="flex shrink-0 items-center gap-2">
            <NikkiPanelButton />
            <NikkiUpdateButton />
            <ThemeToggle />
          </div>
        </div>
      </header>

      <div className="flex min-h-0 w-full flex-1 flex-col lg:flex-row">
        <main className="min-h-0 min-w-0 flex-1 overflow-y-auto overscroll-contain lg:border-r lg:border-edge">
          <Routes>
            <Route path="/" element={<OverviewPage />} />
            <Route path="/groups" element={<GroupsPage />} />
            <Route path="/rulesets" element={<RulesetsPage />} />
            <Route path="/lan-routes" element={<LanRoutesPage />} />
            <Route path="/landings" element={<LandingsPage />} />
            <Route path="/ai" element={<Navigate to="/" replace />} />
          </Routes>
        </main>

        <div className="flex h-[min(40vh,24rem)] w-full shrink-0 flex-col border-t border-edge lg:h-auto lg:w-[min(26rem,32vw)] lg:min-w-[20rem] lg:max-w-md lg:border-t-0 lg:self-stretch">
          <AiPage />
        </div>
      </div>
    </div>
  );
}
