const GPU_ECC_METRIC = /^gpu\.(?:0|[1-9]\d*)\.ecc_dbe$/;
const GPU_XID_METRIC = /^gpu\.(?:0|[1-9]\d*)\.xid$/;
const GPU_PHYSICAL_SENSOR_METRIC = /^gpu\.(?:0|[1-9]\d*)\.(?:temp|mem_temp|power_w)$/;
const METRIC_THRESHOLD_RULE = /^metric_threshold:[a-z0-9]+(?:-[a-z0-9]+)*$/;

export function isPhysicalHardwareFaultEvidence(rule: string, metric: string): boolean {
  if (rule === "hardware_ecc") return GPU_ECC_METRIC.test(metric);
  if (rule === "hardware_xid") return metric === "gpu.xid_last" || GPU_XID_METRIC.test(metric);
  if (!METRIC_THRESHOLD_RULE.test(rule)) return false;
  return metric === "temp.celsius"
    || metric === "power.psu_watts"
    || metric === "power.gpu_watts"
    || GPU_PHYSICAL_SENSOR_METRIC.test(metric);
}
