"use client";

import { useEffect, useMemo, useRef, type ReactNode } from "react";
import { Input } from "../ui/input";
import { InlineNotice } from "./inline-notice";

interface FieldSchema {
  type?: string;
  title?: string;
  description?: string;
  default?: unknown;
  enum?: unknown[];
  minimum?: number;
  maximum?: number;
}

interface ObjectSchema {
  type?: string;
  properties?: Record<string, FieldSchema>;
  required?: string[];
  additionalProperties?: boolean;
}

export interface SchemaChoice {
  value: string;
  label: string;
}

function defaultValues(schema: ObjectSchema): Record<string, unknown> {
  const values: Record<string, unknown> = {};
  for (const [id, field] of Object.entries(schema.properties ?? {})) {
    if (field.default !== undefined) values[id] = field.default;
    else if (field.type === "boolean") values[id] = false;
  }
  return values;
}

function supported(schema: ObjectSchema): string | null {
  if (schema.type !== "object" || !schema.properties) return "Action input schema must declare an object with properties.";
  if (schema.additionalProperties !== false && schema.additionalProperties !== undefined) return "Open-ended action arguments are not supported by the typed form.";
  for (const [id, field] of Object.entries(schema.properties)) {
    if (!["string", "integer", "number", "boolean"].includes(field.type ?? "")) return `Field ${id} uses unsupported type ${field.type ?? "unknown"}.`;
    if (field.enum && !field.enum.every((value) => ["string", "number", "boolean"].includes(typeof value))) return `Field ${id} contains a non-scalar enum.`;
  }
  return null;
}

export function SchemaForm({ schema: rawSchema, values, onChange, choices, choicePlaceholders, footer }: {
  schema: Record<string, unknown>;
  values: Record<string, unknown>;
  onChange: (values: Record<string, unknown>) => void;
  choices?: Record<string, SchemaChoice[]>;
  choicePlaceholders?: Record<string, string>;
  footer?: ReactNode;
}) {
  const schema = rawSchema as ObjectSchema;
  const reason = useMemo(() => supported(schema), [schema]);
  const initialized = useRef(false);
  useEffect(() => {
    if (!reason && !initialized.current) {
      initialized.current = true;
      onChange(defaultValues(schema));
    }
  }, [onChange, reason, schema, values]);
  if (reason) return <InlineNotice tone="warn" title="Incompatible action form" details={reason}>The Bundle must publish a bounded scalar JSON Schema before this action can run here.</InlineNotice>;
  const required = new Set(schema.required ?? []);
  return (
    <div className="space-y-3">
      {Object.entries(schema.properties ?? {}).map(([id, field]) => {
        const fieldChoices = choices?.[id];
        let control: ReactNode;

        if (field.type === "boolean") {
          control = (
            <input
              type="checkbox"
              checked={Boolean(values[id])}
              onChange={(event) => onChange({ ...values, [id]: event.target.checked })}
            />
          );
        } else if (fieldChoices) {
          control = (
            <select
              className="h-9 w-full rounded border border-zinc-800 bg-zinc-950 px-3"
              value={String(values[id] ?? "")}
              onChange={(event) => onChange({ ...values, [id]: event.target.value })}
            >
              <option value="">{choicePlaceholders?.[id] ?? "Select…"}</option>
              {fieldChoices.map((option) => (
                <option key={option.value} value={option.value}>{option.label}</option>
              ))}
            </select>
          );
        } else if (field.enum) {
          control = (
            <select
              className="h-9 w-full rounded border border-zinc-800 bg-zinc-950 px-3"
              value={String(values[id] ?? "")}
              onChange={(event) => {
                const selected = field.enum?.find((value) => String(value) === event.target.value);
                onChange({ ...values, [id]: selected });
              }}
            >
              <option value="">Select…</option>
              {field.enum.map((option) => (
                <option key={String(option)} value={String(option)}>{String(option)}</option>
              ))}
            </select>
          );
        } else {
          control = (
            <Input
              type={field.type === "integer" || field.type === "number" ? "number" : "text"}
              min={field.minimum}
              max={field.maximum}
              value={String(values[id] ?? "")}
              onChange={(event) => onChange({
                ...values,
                [id]: field.type === "integer"
                  ? Number.parseInt(event.target.value, 10)
                  : field.type === "number" ? Number(event.target.value) : event.target.value,
              })}
            />
          );
        }

        return (
          <label key={id} className="block space-y-1 text-xs text-zinc-300">
            <span>{field.title || id}{required.has(id) ? " *" : ""}</span>
            {control}
            {field.description && <span className="sr-only">{field.description}</span>}
          </label>
        );
      })}
      {footer}
    </div>
  );
}
