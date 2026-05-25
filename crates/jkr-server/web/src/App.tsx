import { useQuery } from "@tanstack/react-query";
import { useState } from "react";
import { api, type Me, ApiError } from "./api";
import { LandingView } from "./views/Landing";
import { LoginView } from "./views/Login";
import { DashboardView } from "./views/Dashboard";
import { SessionDetail } from "./views/SessionDetail";

type Route =
  | { name: "auto" }
  | { name: "login" }
  | { name: "session"; id: string };

export function App() {
  const [route, setRoute] = useState<Route>({ name: "auto" });

  const meQuery = useQuery<Me, ApiError>({
    queryKey: ["me"],
    queryFn: () => api<Me>("/api/auth/me"),
    retry: false,
  });

  if (meQuery.isLoading) return null;

  // Explicit route picks take precedence.
  if (route.name === "login") {
    return <LoginView />;
  }

  // Authenticated paths.
  if (meQuery.data) {
    if (route.name === "session") {
      return (
        <SessionDetail
          id={route.id}
          onBack={() => setRoute({ name: "auto" })}
        />
      );
    }
    return (
      <DashboardView
        me={meQuery.data}
        onSelectSession={(id) => setRoute({ name: "session", id })}
        onSignedOut={() => meQuery.refetch()}
      />
    );
  }

  // Unauthenticated: show landing page.
  return <LandingView onSignIn={() => setRoute({ name: "login" })} />;
}
