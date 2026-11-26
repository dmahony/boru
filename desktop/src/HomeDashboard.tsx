import type { AppSnapshotDto } from "./contract";

export function HomeDashboard({ snapshot, onOpenRoom, onOpenMedia }: { snapshot: AppSnapshotDto; onOpenRoom: () => void; onOpenMedia: () => void }) {
  const health = snapshot.network_health;
  const peers = health?.connected_peer_count;
  const activity = snapshot.activity.slice(0, 4);
  return <section className="home-dashboard" aria-label="Home dashboard">
    <header className="home-hero"><div><p className="eyebrow">BORU · PEER-TO-PEER</p><h2>Welcome{snapshot.identity?.display_name ? `, ${snapshot.identity.display_name}` : " back"}</h2><p>Your conversations and network, at a glance.</p></div><span className={health?.ready ? "health-pill online" : "health-pill offline"}>{health ? health.status : "Network status unavailable"}</span></header>
    <nav className="home-actions" aria-label="Quick actions"><button onClick={onOpenRoom}>Open conversations <span aria-hidden="true">→</span></button><button onClick={onOpenMedia}>Share a file <span aria-hidden="true">→</span></button></nav>
    <div className="home-grid">
      <article className="home-card"><h3>People & conversations</h3><strong>{snapshot.direct_messages.length}</strong><p>Direct conversations</p><span>{snapshot.rooms.length} rooms · {snapshot.unread.reduce((sum, item) => sum + item.count, 0)} unread</span></article>
      <article className="home-card"><h3>Network health</h3><strong>{health ? (peers === null || peers === undefined ? "—" : peers) : "—"}</strong><p>{health ? (peers === null || peers === undefined ? "Connected peer count unavailable" : `connected ${peers === 1 ? "peer" : "peers"}`) : "Waiting for network snapshot"}</p><span>{health?.relay_url ? `Relay · ${health.relay_url}` : health?.ready ? "Direct or relay connectivity" : "Offline or not yet connected"}</span></article>
      <article className="home-card"><h3>File transfers</h3><strong>{snapshot.transfers.filter(item => ["active", "verifying"].includes(item.state)).length}</strong><p>In progress</p><span>{snapshot.transfers.length} active/recent transfer records</span></article>
      <article className="home-card"><h3>Tunnels</h3><strong>—</strong><p>Tunnel service snapshot unavailable</p><span>No tunnel count is shown until the shared runtime projection is available.</span></article>
      <article className="home-card home-activity"><h3>Recent activity</h3>{activity.length ? <ul>{activity.map(item => <li key={item.id}><span>{item.summary}</span><time dateTime={new Date(item.occurred_at_ms).toISOString()}>{new Date(item.occurred_at_ms).toLocaleString()}</time></li>)}</ul> : <p className="empty">No recent activity yet.</p>}</article>
    </div>
  </section>;
}

