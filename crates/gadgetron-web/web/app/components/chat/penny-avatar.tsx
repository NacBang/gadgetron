"use client";

import { Avatar, AvatarFallback, AvatarImage } from "@/components/ui/avatar";
import { cn } from "@/lib/utils";

export function PennyAvatar({
  className,
  size = "default",
}: {
  className?: string;
  size?: "default" | "sm" | "lg";
}) {
  return (
    <Avatar className={cn("border border-zinc-800", className)} size={size}>
      <AvatarImage src="/web/brand/penny.png" alt="" />
      <AvatarFallback className="bg-zinc-900 text-[10px] font-bold text-zinc-300">
        P
      </AvatarFallback>
    </Avatar>
  );
}
