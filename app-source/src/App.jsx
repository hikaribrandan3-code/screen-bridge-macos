import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { STRINGS, detectLang } from './i18n';

const GearIcon = () => (
  <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round">
    <circle cx="12" cy="12" r="3" />
    <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 1 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 1 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 1 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 1 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z" />
  </svg>
);

const DeviceIcon = ({ kind }) => (
  <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round">
    {kind === 'android'
      ? <><rect x="5" y="2" width="14" height="20" rx="2.5" /><line x1="10" y1="19" x2="14" y2="19" /></>
      : <><rect x="4" y="3" width="16" height="18" rx="2.5" /><line x1="11" y1="18" x2="13" y2="18" /></>}
  </svg>
);

const SignalBars = ({ quality }) => {
  const bars = quality === 'streaming' ? 4 : quality === 'waiting' ? 2 : 1;
  return (
    <div style={{ display: 'flex', gap: '2px', alignItems: 'flex-end' }}>
      {[1, 2, 3, 4].map((i) => (
        <div key={i} style={{
          width: '2px',
          height: `${i * 3}px`,
          background: i <= bars ? 'var(--green)' : 'var(--gray)',
          borderRadius: '1px'
        }} />
      ))}
    </div>
  );
};

export default function App() {
  const [langPref, setLangPref] = useState('auto');
  const lang = langPref === 'auto' ? detectLang() : langPref;
  const t = useMemo(() => STRINGS[lang], [lang]);

  const [devices, setDevices] = useState([]);
  const [selectedDevice, setSelectedDevice] = useState(() => localStorage.getItem('lastDevice') || '');
  const [mode, setMode] = useState('wifi');
  const [phase, setPhase] = useState('idle'); // idle | connecting | waiting | streaming
  const [session, setSession] = useState(null); // { url, qr_svg }
  const [clients, setClients] = useState(0);
  const [error, setError] = useState(null);
  const [showSettings, setShowSettings] = useState(false);
  const [quality, setQuality] = useState('medium');
  const [audio, setAudio] = useState('both');       // both | tablet | off
  const [resolution, setResolution] = useState('retina'); // retina | standard
  const [connectionLatency, setConnectionLatency] = useState(0);
  const pollRef = useRef(null);

  const refreshDevices = useCallback(async () => {
    try {
      const list = await invoke('list_devices');
      setDevices(list);
      setSelectedDevice((cur) => {
        const saved = localStorage.getItem('lastDevice') || '';
        const valid = cur && list.some((d) => d.name === cur) ? cur : (list[0]?.name || saved);
        return valid;
      });
    } catch { /* backend not ready yet */ }
  }, []);

  const handleDeviceChange = (newDevice) => {
    setSelectedDevice(newDevice);
    localStorage.setItem('lastDevice', newDevice);
  };

  useEffect(() => {
    refreshDevices();
    pollRef.current = setInterval(refreshDevices, 3000);
    const unlisten = listen('clients', (e) => {
      const n = Number(e.payload) || 0;
      setClients(n);
      setPhase((p) => (p === 'waiting' || p === 'streaming') ? (n > 0 ? 'streaming' : 'waiting') : p);
    });
    const unlistenError = listen('session-error', async (e) => {
      const message = String(e.payload || 'Unknown local server error');
      setError(message.includes('virtual display') ? t.errDisplay + ' (' + message + ')' : t.errGeneric + ' (' + message + ')');
      try { await invoke('disconnect_display'); } catch { /* session already ended */ }
      setSession(null);
      setClients(0);
      setPhase('idle');
    });
    return () => { clearInterval(pollRef.current); unlisten.then((f) => f()); unlistenError.then((f) => f()); };
  }, [refreshDevices, t.errDisplay, t.errGeneric]);

  const connect = async () => {
    setError(null);
    setPhase('connecting');
    try {
      const info = await invoke('connect_display', { mode, quality, audio, resolution });
      setSession(info);
      setPhase('waiting');
    } catch (e) {
      const msg = String(e);
      if (msg.includes('permission')) setError(t.errPermission);
      else if (msg.includes('virtual display')) setError(t.errDisplay);
      else setError(t.errGeneric + ' (' + msg + ')');
      setPhase('idle');
    }
  };

  const disconnect = async () => {
    try { await invoke('disconnect_display'); } catch { /* already down */ }
    setSession(null);
    setClients(0);
    setPhase('idle');
  };

  const connected = phase === 'waiting' || phase === 'streaming';
  const statusColor = phase === 'streaming' ? 'var(--green)' : connected ? 'var(--blue)' : phase === 'connecting' ? 'var(--orange)' : 'var(--green)';
  const statusText = phase === 'streaming' ? t.streaming
    : phase === 'waiting' ? t.waitingTablet
    : phase === 'connecting' ? t.connecting
    : t.ready;

  return (
    <div className="window">
      <button className="gear" title={t.settings} onClick={() => setShowSettings((s) => !s)}><GearIcon /></button>

      {showSettings && (
        <div className="settings-panel">
          <div className="settings-title">{t.settings}</div>
          <label>{t.quality}
            <select value={quality} onChange={(e) => setQuality(e.target.value)} disabled={connected}>
              <option value="low">{t.qLow}</option>
              <option value="medium">{t.qMed}</option>
              <option value="high">{t.qHigh}</option>
            </select>
          </label>
          <label>{t.resolution}
            <select value={resolution} onChange={(e) => setResolution(e.target.value)} disabled={connected}>
              <option value="retina">{t.rRetina}</option>
              <option value="standard">{t.rStd}</option>
            </select>
          </label>
          <label>{t.audioRoute}
            <select value={audio} onChange={(e) => setAudio(e.target.value)} disabled={connected}>
              <option value="both">{t.aBoth}</option>
              <option value="tablet">{t.aTablet}</option>
              <option value="off">{t.aOff}</option>
            </select>
          </label>
          <label>{t.language}
            <select value={langPref} onChange={(e) => setLangPref(e.target.value)}>
              <option value="auto">{t.auto}</option>
              <option value="es">Español</option>
              <option value="en">English</option>
            </select>
          </label>
        </div>
      )}

      <main className="stack">
        {!connected ? (
          <button className="connect-btn" onClick={connect} disabled={phase === 'connecting'}>
            {phase === 'connecting' ? t.connecting : t.connect}
          </button>
        ) : (
          <button className="connect-btn disconnect" onClick={disconnect}>{t.disconnect}</button>
        )}

        {!connected && (
          <>
            <div className="field-label">{t.selectDevice}</div>
            <div className="device-select">
              {devices.length === 0 ? (
                <div className="device-empty">{t.noDevices}</div>
              ) : (
                <select value={selectedDevice} onChange={(e) => handleDeviceChange(e.target.value)}>
                  {devices.map((d) => (
                    <option key={d.name} value={d.name}>{d.name}</option>
                  ))}
                </select>
              )}
              {devices.length > 0 && <span className="device-icon"><DeviceIcon kind={devices.find((d) => d.name === selectedDevice)?.kind} /></span>}
            </div>

            <div className="field-label">{t.connectionMode}</div>
            <div className="mode-toggle">
              <button className={mode === 'wifi' ? 'active' : ''} onClick={() => setMode('wifi')}>{t.wifi}</button>
              <button className={mode === 'cable' ? 'active' : ''} onClick={() => setMode('cable')}>{t.cable}</button>
            </div>
          </>
        )}

        {connected && session && (
          <div className="session-card">
            <div className="url-label">{t.openOnTablet}</div>
            <div className="url">{session.url}</div>
            {mode === 'cable' && <div className="hint">{t.cableHint}</div>}
            {session.qr_svg && (
              <>
                <div className="qr-label">{t.scanQr}</div>
                <div className="qr" dangerouslySetInnerHTML={{ __html: session.qr_svg }} />
              </>
            )}
            {phase === 'streaming' && <div className="clients">{clients} {t.clients}</div>}
            {audio !== 'off' && (
              <div className="hint" style={{ color: session.audio_ok ? 'var(--green)' : 'var(--orange)' }}>
                {session.audio_ok ? '🔊 ' + t.audioOn : '🔇 ' + t.audioUnavailable}
              </div>
            )}
          </div>
        )}

        <div className="status" style={{ display: 'flex', alignItems: 'center', gap: '8px' }}>
          <span className="dot" style={{ background: statusColor, boxShadow: `0 0 6px ${statusColor}` }} />
          {statusText}
          {connected && <SignalBars quality={phase} />}
        </div>

        {error && <div className="error">{error}</div>}
      </main>
    </div>
  );
}
