export {};
if (new URLSearchParams(location.search).has('demo')) await import('../demo/main');
else await import('./main');
