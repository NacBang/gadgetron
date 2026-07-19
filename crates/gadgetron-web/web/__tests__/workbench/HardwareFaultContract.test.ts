import { describe, expect, it } from "vitest";

import { isPhysicalHardwareFaultEvidence } from "../../e2e/physical-hardware-fault-contract";

describe("physical hardware fault evidence", () => {
  it.each([
    ["hardware_ecc", "gpu.0.ecc_dbe"],
    ["hardware_xid", "gpu.3.xid"],
    ["hardware_xid", "gpu.xid_last"],
    ["metric_threshold:host-temperature", "temp.celsius"],
    ["metric_threshold:gpu-temperature", "gpu.0.temp"],
    ["metric_threshold:gpu-memory-temperature", "gpu.12.mem_temp"],
    ["metric_threshold:gpu-power", "gpu.2.power_w"],
    ["metric_threshold:psu-power", "power.psu_watts"],
    ["metric_threshold:gpu-total-power", "power.gpu_watts"],
  ])("accepts collector physical evidence %s / %s", (rule, metric) => {
    expect(isPhysicalHardwareFaultEvidence(rule, metric)).toBe(true);
  });

  it.each([
    ["", "gpu.0.ecc_dbe"],
    ["hardware_ecc", "gpu.0.xid"],
    ["hardware_xid", "gpu.0.ecc_dbe"],
    ["metric_threshold:cpu-load", "cpu.util_percent"],
    ["metric_threshold:memory", "mem.used_percent"],
    ["metric_threshold:disk", "disk.used_percent"],
    ["metric_threshold:gpu-util", "gpu.0.util"],
    ["metric_threshold:network", "nic.eth0.rx_bps"],
    ["metric_threshold:", "temp.celsius"],
    ["metric_threshold:GPU-temperature", "gpu.0.temp"],
  ])("rejects non-physical or mismatched evidence %s / %s", (rule, metric) => {
    expect(isPhysicalHardwareFaultEvidence(rule, metric)).toBe(false);
  });
});
