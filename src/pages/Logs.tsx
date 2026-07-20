import { useMeterSettingsStore } from "@/stores/useMeterSettingsStore";
import "./Logs.css";

import { AppShell, Burger, Group, NavLink, Text } from "@mantine/core";
import { useDisclosure } from "@mantine/hooks";
import { Gear, House } from "@phosphor-icons/react";
import { listen } from "@tauri-apps/api/event";
import { useEffect, useRef } from "react";
import { Toaster } from "react-hot-toast";
import { Link, Outlet, useNavigate } from "react-router-dom";

const MAX_DEBUG_CONSOLE_EVENTS = 500;

const Layout = () => {
  const [mobileOpened, { toggle: toggleMobile }] = useDisclosure();
  const [desktopOpened, { toggle: toggleDesktop }] = useDisclosure(false);
  const { open_log_on_save } = useMeterSettingsStore((state) => ({ open_log_on_save: state.open_log_on_save }));

  const navigate = useNavigate();
  const debugEventCount = useRef(0);

  useEffect(() => {
    const debugListener = listen("debug-event", (event: { payload: unknown }) => {
      const eventIndex = debugEventCount.current++;
      if (eventIndex < MAX_DEBUG_CONSOLE_EVENTS) {
        console.info(JSON.stringify(event.payload));
      } else if (eventIndex === MAX_DEBUG_CONSOLE_EVENTS) {
        console.warn(`Debug event console limit reached (${MAX_DEBUG_CONSOLE_EVENTS}); further events are suppressed.`);
      }
    });

    return () => {
      debugListener.then((f) => f());
    };
  }, []);

  useEffect(() => {
    const saveListener = listen("encounter-saved", (event: { payload: number | null }) => {
      if (event.payload && open_log_on_save) {
        navigate(`/logs/${event.payload}`);
      }
    });

    return () => {
      saveListener.then((f) => f());
    };
  }, [navigate, open_log_on_save]);

  return (
    <div className="log-window">
      <AppShell
        header={{ height: 50 }}
        navbar={{
          width: 300,
          breakpoint: "sm",
          collapsed: { mobile: !mobileOpened, desktop: !desktopOpened },
        }}
        padding="sm"
      >
        <AppShell.Header>
          <Group h="100%" px="sm">
            <Burger opened={mobileOpened} onClick={toggleMobile} hiddenFrom="sm" size="sm" />
            <Burger opened={desktopOpened} onClick={toggleDesktop} visibleFrom="sm" size="sm" />
            <Text>GBFR Logs Awa Edition</Text>
          </Group>
        </AppShell.Header>
        <AppShell.Navbar p="sm">
          <AppShell.Section grow>
            <NavLink label="Logs" leftSection={<House size="1rem" />} component={Link} to="/logs" />
          </AppShell.Section>
          <AppShell.Section>
            <NavLink label="Settings" leftSection={<Gear size="1rem" />} component={Link} to="/logs/settings" />
          </AppShell.Section>
        </AppShell.Navbar>
        <AppShell.Main>
          <Outlet />
        </AppShell.Main>
      </AppShell>
      <Toaster
        position="bottom-center"
        toastOptions={{
          style: {
            borderRadius: "10px",
            backgroundColor: "#252525",
            color: "#fff",
            fontSize: "14px",
          },
        }}
      />
    </div>
  );
};

export default Layout;
