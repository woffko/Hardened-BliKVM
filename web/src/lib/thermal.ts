export function formatTemperatureMillidegrees(value: number | null | undefined) {
  if (value === null || value === undefined || !Number.isInteger(value)) return '-';
  if (value < -40_000 || value > 150_000) return '-';
  return `${(value / 1000).toFixed(1)} °C`;
}
