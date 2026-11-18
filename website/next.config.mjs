export default {
  output: "export",
  experimental: { globalNotFound: true, agentUpgrade: "security" },
  trailingSlash: true,
  images: { unoptimized: true },
  agentRules: false,
  devIndicators: false,
  turbopack: { root: import.meta.dirname },
};
