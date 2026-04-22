"use client";

import { useCallback, useEffect, useState } from "react";
import { Toaster, toast } from "sonner";
import { Button } from "../../components/ui/button";
import { Input } from "../../components/ui/input";
import { useAuth } from "../../lib/auth-context";

// ---------------------------------------------------------------------------
// /web/admin — user management page.
//
// First iteration: list users (email / display_name / role) + add-user form.
// "Group" column shows role today; a proper team/group concept will swap
// in later when the teams table is exposed through this page too.
// ---------------------------------------------------------------------------

function getApiBase(): string {
  if (typeof document === "undefined") return "/api/v1/web";
  const meta = document.querySelector<HTMLMetaElement>(
    'meta[name="gadgetron-api-base"]',
  );
  const chatBase = meta?.content || "/v1";
  return chatBase.replace(/\/v1$/, "/api/v1/web");
}

interface UserRow {
  id: string;
  email: string;
  display_name: string;
  role: "member" | "admin" | "service";
  is_active: boolean;
  created_at: string;
}

interface ListResponse {
  users: UserRow[];
  returned: number;
}

function authHeaders(apiKey: string | null): Record<string, string> {
  return apiKey ? { Authorization: `Bearer ${apiKey}` } : {};
}

async function listUsers(apiKey: string | null): Promise<UserRow[]> {
  const res = await fetch(`${getApiBase()}/workbench/admin/users?limit=500`, {
    credentials: "include",
    headers: authHeaders(apiKey),
  });
  if (!res.ok) {
    throw new Error(`list users: HTTP ${res.status}`);
  }
  const body = (await res.json()) as ListResponse;
  return body.users;
}

async function createUser(
  apiKey: string | null,
  body: {
    email: string;
    display_name: string;
    role: "member" | "admin";
    password: string;
  },
): Promise<UserRow> {
  const res = await fetch(`${getApiBase()}/workbench/admin/users`, {
    method: "POST",
    credentials: "include",
    headers: {
      ...authHeaders(apiKey),
      "Content-Type": "application/json",
    },
    body: JSON.stringify(body),
  });
  if (!res.ok) {
    const text = await res.text();
    throw new Error(`create user: HTTP ${res.status} ${text}`);
  }
  return (await res.json()) as UserRow;
}

async function deleteUser(
  apiKey: string | null,
  userId: string,
): Promise<void> {
  const res = await fetch(
    `${getApiBase()}/workbench/admin/users/${userId}`,
    {
      method: "DELETE",
      credentials: "include",
      headers: authHeaders(apiKey),
    },
  );
  if (!res.ok) {
    const text = await res.text();
    throw new Error(`delete user: HTTP ${res.status} ${text}`);
  }
}

// ---------------------------------------------------------------------------
// AddUserForm — collapsible block above the table.
// ---------------------------------------------------------------------------

function AddUserForm({
  apiKey,
  onAdded,
}: {
  apiKey: string | null;
  onAdded: () => void;
}) {
  const [email, setEmail] = useState("");
  const [name, setName] = useState("");
  const [role, setRole] = useState<"member" | "admin">("member");
  const [password, setPassword] = useState("");
  const [busy, setBusy] = useState(false);

  const submit = useCallback(async () => {
    if (!email.trim() || !name.trim() || !password.trim()) {
      toast.error("email, name, password 모두 필수");
      return;
    }
    setBusy(true);
    try {
      await createUser(apiKey, {
        email: email.trim(),
        display_name: name.trim(),
        role,
        password,
      });
      toast.success(`유저 생성: ${email}`);
      setEmail("");
      setName("");
      setPassword("");
      setRole("member");
      onAdded();
    } catch (e) {
      toast.error((e as Error).message);
    } finally {
      setBusy(false);
    }
  }, [apiKey, email, name, role, password, onAdded]);

  return (
    <section className="rounded border border-zinc-800 bg-zinc-900 p-4">
      <h2 className="mb-3 text-sm font-medium text-zinc-200">새 유저 추가</h2>
      <div className="grid grid-cols-1 gap-3 md:grid-cols-2 lg:grid-cols-5">
        <div className="lg:col-span-2">
          <label className="mb-1 block text-[11px] text-zinc-500">Email</label>
          <Input
            type="email"
            value={email}
            onChange={(e) => setEmail(e.target.value)}
            placeholder="alice@example.com"
            autoComplete="off"
          />
        </div>
        <div>
          <label className="mb-1 block text-[11px] text-zinc-500">이름</label>
          <Input
            value={name}
            onChange={(e) => setName(e.target.value)}
            placeholder="Alice Kim"
            autoComplete="off"
          />
        </div>
        <div>
          <label className="mb-1 block text-[11px] text-zinc-500">그룹</label>
          <select
            value={role}
            onChange={(e) => setRole(e.target.value as "member" | "admin")}
            className="flex h-9 w-full rounded-md border border-zinc-700 bg-zinc-950 px-2 text-sm text-zinc-200"
          >
            <option value="member">member</option>
            <option value="admin">admin</option>
          </select>
        </div>
        <div>
          <label className="mb-1 block text-[11px] text-zinc-500">임시 비밀번호</label>
          <Input
            type="text"
            value={password}
            onChange={(e) => setPassword(e.target.value)}
            placeholder="temporary"
            autoComplete="new-password"
          />
        </div>
      </div>
      <div className="mt-3 flex justify-end">
        <Button onClick={() => void submit()} disabled={busy}>
          {busy ? "추가 중…" : "추가"}
        </Button>
      </div>
    </section>
  );
}

// ---------------------------------------------------------------------------
// UsersTable
// ---------------------------------------------------------------------------

function UsersTable({
  users,
  apiKey,
  onDeleted,
}: {
  users: UserRow[];
  apiKey: string | null;
  onDeleted: () => void;
}) {
  const [deleting, setDeleting] = useState<string | null>(null);
  const remove = useCallback(
    async (u: UserRow) => {
      if (!window.confirm(`${u.email} 삭제?`)) return;
      setDeleting(u.id);
      try {
        await deleteUser(apiKey, u.id);
        toast.success(`삭제: ${u.email}`);
        onDeleted();
      } catch (e) {
        toast.error((e as Error).message);
      } finally {
        setDeleting(null);
      }
    },
    [apiKey, onDeleted],
  );

  return (
    <section className="rounded border border-zinc-800 bg-zinc-900">
      <header className="flex items-center justify-between border-b border-zinc-800 px-4 py-2">
        <h2 className="text-sm font-medium text-zinc-200">
          유저 목록 <span className="text-zinc-500">({users.length})</span>
        </h2>
      </header>
      <table className="w-full text-sm">
        <thead className="bg-zinc-950 text-[11px] uppercase text-zinc-500">
          <tr>
            <th className="px-4 py-2 text-left font-normal">Email</th>
            <th className="px-4 py-2 text-left font-normal">이름</th>
            <th className="px-4 py-2 text-left font-normal">그룹</th>
            <th className="w-24 px-4 py-2 text-right font-normal"></th>
          </tr>
        </thead>
        <tbody>
          {users.map((u) => (
            <tr
              key={u.id}
              className="border-t border-zinc-800 text-zinc-300 hover:bg-zinc-950/50"
            >
              <td className="px-4 py-2 font-mono text-xs">{u.email}</td>
              <td className="px-4 py-2">{u.display_name}</td>
              <td className="px-4 py-2">
                <span
                  className={
                    u.role === "admin"
                      ? "rounded bg-amber-950/40 px-1.5 py-0.5 text-[10px] text-amber-300"
                      : u.role === "service"
                        ? "rounded bg-zinc-800 px-1.5 py-0.5 text-[10px] text-zinc-400"
                        : "rounded bg-blue-950/40 px-1.5 py-0.5 text-[10px] text-blue-300"
                  }
                >
                  {u.role}
                </span>
              </td>
              <td className="px-4 py-2 text-right">
                <Button
                  size="sm"
                  variant="ghost"
                  className="h-6 px-2 text-[11px] text-red-400 hover:text-red-300"
                  disabled={deleting === u.id}
                  onClick={() => void remove(u)}
                >
                  {deleting === u.id ? "…" : "삭제"}
                </Button>
              </td>
            </tr>
          ))}
          {users.length === 0 && (
            <tr>
              <td
                colSpan={4}
                className="px-4 py-6 text-center text-[11px] text-zinc-600"
              >
                등록된 유저가 없습니다.
              </td>
            </tr>
          )}
        </tbody>
      </table>
    </section>
  );
}

// ---------------------------------------------------------------------------
// Page
// ---------------------------------------------------------------------------

function ApiKeyOverride({
  onSet,
}: {
  onSet: (k: string) => void;
}) {
  const [value, setValue] = useState("");
  return (
    <div className="rounded border border-amber-900/60 bg-amber-950/20 p-3">
      <p className="text-[11px] text-amber-300">
        현재 저장된 API 키가 Management scope를 갖고 있지 않습니다. 관리자 키로
        교체하세요 (CLI에서 생성: <code className="font-mono">gadgetron key create --scope "OpenAiCompat,Management"</code>).
      </p>
      <div className="mt-2 flex gap-2">
        <Input
          type="password"
          value={value}
          onChange={(e) => setValue(e.target.value)}
          placeholder="gad_live_..."
          autoComplete="off"
          className="flex-1"
        />
        <Button
          size="sm"
          onClick={() => {
            if (value.trim()) onSet(value.trim());
          }}
        >
          교체
        </Button>
      </div>
    </div>
  );
}

export default function AdminPage() {
  const { apiKey, saveKey, identity } = useAuth();
  const [users, setUsers] = useState<UserRow[]>([]);
  const [loading, setLoading] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  // Either an API key OR a logged-in session grants access; the
  // backend middleware accepts the session cookie when Bearer is absent.
  const canCall = !!apiKey || !!identity;

  const refresh = useCallback(async () => {
    if (!canCall) return;
    setLoading(true);
    setErr(null);
    try {
      const rows = await listUsers(apiKey);
      setUsers(rows);
    } catch (e) {
      setErr((e as Error).message);
    } finally {
      setLoading(false);
    }
  }, [apiKey, canCall]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  return (
    <div className="h-full overflow-y-auto">
      <div className="mx-auto max-w-5xl space-y-4 p-6">
        <header className="flex items-center justify-between">
          <div>
            <h1 className="text-lg font-semibold text-zinc-100">Admin</h1>
            <p className="text-[11px] text-zinc-500">
              유저 관리 — Management scope 필요
            </p>
          </div>
          <Button
            variant="ghost"
            size="sm"
            onClick={() => void refresh()}
            disabled={loading}
            className="h-7 px-2 text-[11px]"
          >
            {loading ? "…" : "Refresh"}
          </Button>
        </header>

        {!canCall && (
          <div className="rounded border border-amber-900/60 bg-amber-950/40 px-3 py-2 text-[11px] text-amber-300">
            로그인이 필요합니다.
          </div>
        )}

        {err && (
          <div className="rounded border border-red-900/60 bg-red-950/40 px-3 py-2 text-[11px] text-red-300">
            {err}
          </div>
        )}

        {err && err.includes("403") && (
          <ApiKeyOverride onSet={(k) => saveKey(k)} />
        )}

        <AddUserForm apiKey={apiKey} onAdded={refresh} />
        <UsersTable users={users} apiKey={apiKey} onDeleted={refresh} />
      </div>
      <Toaster theme="dark" position="top-right" richColors />
    </div>
  );
}
