// electron-builder afterPack hook: flips Electron's fuses in the packaged
// binary (security lead, B1 point 2). test/fuses.test.mjs reads them back.
'use strict';

const path = require('node:path');
const { flipFuses, FuseVersion, FuseV1Options } = require('@electron/fuses');

const FUSES = {
  version: FuseVersion.V1,
  [FuseV1Options.RunAsNode]: false,
  [FuseV1Options.EnableNodeOptionsEnvironmentVariable]: false,
  [FuseV1Options.EnableNodeCliInspectArguments]: false,
  [FuseV1Options.OnlyLoadAppFromAsar]: true,
  [FuseV1Options.EnableEmbeddedAsarIntegrityValidation]: true,
  [FuseV1Options.EnableCookieEncryption]: true,
  [FuseV1Options.GrantFileProtocolExtraPrivileges]: false,
};

function binaryOf(context) {
  const name = context.packager.appInfo.productFilename;
  const exe = context.packager.executableName || name;
  switch (context.electronPlatformName) {
    case 'darwin':
      return path.join(context.appOutDir, `${name}.app`);
    case 'win32':
      return path.join(context.appOutDir, `${name}.exe`);
    default:
      return path.join(context.appOutDir, exe);
  }
}

module.exports = async function afterPack(context) {
  await flipFuses(binaryOf(context), { ...FUSES, resetAdHocDarwinSignature: context.electronPlatformName === 'darwin' });
};
module.exports.FUSES = FUSES;
