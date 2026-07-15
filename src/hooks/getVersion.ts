import packageInfo from "../../package.json";

export default function getVersion() {
  return {
    version: packageInfo.version,
  };
}
