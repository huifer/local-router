import { Toaster as Sonner } from "sonner";

function Toaster() {
  return (
    <Sonner
      theme="dark"
      position="bottom-right"
      toastOptions={{
        style: {
          background: "#151c1b",
          border: "1px solid #344340",
          color: "#edf2f0",
          fontSize: "12px",
        },
      }}
    />
  );
}

export { Toaster };
