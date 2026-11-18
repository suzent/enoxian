"use client";
import { useEffect, useRef } from "react";
import {
  circleFamily,
  drawCircleMark,
  type CircleFamily,
} from "../shared-circle-identity";

export function CircleBadge({
  circleId,
  size = 28,
  family,
  active = false,
}: {
  circleId: string;
  size?: number;
  family?: CircleFamily;
  active?: boolean;
}) {
  const ref = useRef<HTMLCanvasElement>(null);

  useEffect(() => {
    const canvas = ref.current;
    const ctx = canvas?.getContext("2d");
    if (!canvas || !ctx) return;

    const render = () => {
      const dpr = Math.min(window.devicePixelRatio || 1, 2);
      canvas.width = size * dpr;
      canvas.height = size * dpr;
      ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
      ctx.clearRect(0, 0, size, size);
      const color = getComputedStyle(canvas).color || "#0a0a0a";
      drawCircleMark(
        ctx,
        size / 2,
        size / 2,
        size / 2.7,
        circleId,
        family ?? circleFamily(circleId),
        color,
        1,
        active ? 0.35 : 0,
      );
    };

    render();
    const observer = new MutationObserver(render);
    observer.observe(document.documentElement, {
      attributes: true,
      attributeFilter: ["class", "data-theme"],
    });
    return () => observer.disconnect();
  }, [circleId, size, family, active]);

  return (
    <canvas
      ref={ref}
      style={{ width: size, height: size, display: "block", flexShrink: 0 }}
      aria-hidden="true"
    />
  );
}
