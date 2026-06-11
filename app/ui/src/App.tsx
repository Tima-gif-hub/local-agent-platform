export function App() {
  return (
    <main className="shell">
      <label className="inputWrap" htmlFor="prompt">
        <span>Jarvis</span>
        <input id="prompt" autoFocus placeholder="Type a command..." />
      </label>
    </main>
  );
}
