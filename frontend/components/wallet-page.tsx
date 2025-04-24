"use client";

import { useEffect, useState } from "react";
import { useRouter } from "next/navigation";
import {
  ArrowLeftRight,
  Bitcoin,
  Copy,
  Download,
  Loader2,
  LogOut,
  RefreshCw,
  Send,
  Wallet,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { Separator } from "@/components/ui/separator";
import { SendModal } from "./send-modal";
import { ReceiveModal } from "./receive-modal";
import { toast } from "sonner";

interface WalletBalance {
  wallet_id: string;
  offchain_balance: {
    spendable: number;
    expired: number;
  };
  boarding_balance: {
    spendable: number;
    expired: number;
    pending: number;
  };
}

export function WalletPage() {
  const [walletId, setWalletId] = useState<string | null>(null);
  const [onchainAddress, setOnchainAddress] = useState<string | null>(null);
  const [offchainAddress, setOffchainAddress] = useState<string | null>(null);
  const [balance, setBalance] = useState<WalletBalance | null>(null);
  const [isLoading, setIsLoading] = useState(false);
  const [isSendModalOpen, setIsSendModalOpen] = useState(false);
  const [isReceiveModalOpen, setIsReceiveModalOpen] = useState(false);

  const router = useRouter();

  useEffect(() => {
    // Check if wallet info exists in localStorage
    const storedWalletId = localStorage.getItem("wallet_id");
    const storedOnchainAddress = localStorage.getItem("onchain_address");
    const storedOffchainAddress = localStorage.getItem("offchain_address");

    if (!storedWalletId || !storedOnchainAddress || !storedOffchainAddress) {
      // Redirect to landing page if wallet info is missing
      router.push("/");
      return;
    }

    setWalletId(storedWalletId);
    setOnchainAddress(storedOnchainAddress);
    setOffchainAddress(storedOffchainAddress);

    // Fetch initial balance
    fetchBalance(storedWalletId);
  }, [router]);

  const fetchBalance = async (id: string) => {
    try {
      setIsLoading(true);
      const response = await fetch(`http://localhost:8080/get_balance/${id}`);

      if (!response.ok) {
        throw new Error("Failed to fetch balance");
      }

      const data = await response.json();
      setBalance(data);
    } catch (error) {
      console.error("Error fetching balance:", error);
      toast.error("Error", {
        description: "Failed to fetch wallet balance",
      });
    } finally {
      setIsLoading(false);
    }
  };

  const handleRefresh = () => {
    if (walletId) {
      fetchBalance(walletId);
    }
  };

  const handleFaucet = async () => {
    if (!onchainAddress) return;

    try {
      setIsLoading(true);
      const response = await fetch("http://localhost:8080/faucet", {
        method: "POST",
        headers: {
          "Content-Type": "application/json",
        },
        body: JSON.stringify({
          onchain_address: onchainAddress,
          amount: 0.0001,
        }),
      });

      const data = await response.json();

      if (!data.success) {
        throw new Error(data.error || "Faucet request failed");
      }

      toast.success("Success", {
        description: "Faucet request successful. Funds will appear shortly.",
      });

      // Refresh balance after a short delay
      setTimeout(() => {
        if (walletId) fetchBalance(walletId);
      }, 2000);
    } catch (error) {
      console.error("Faucet error:", error);
      toast.error("Error", {
        description:
          error instanceof Error ? error.message : "Faucet request failed",
      });
    } finally {
      setIsLoading(false);
    }
  };

  const handleSettle = async () => {
    if (!walletId) return;

    try {
      setIsLoading(true);
      const response = await fetch("http://localhost:8080/settle", {
        method: "POST",
        headers: {
          "Content-Type": "application/json",
        },
        body: JSON.stringify({
          wallet_id: walletId,
        }),
      });

      const data = await response.json();

      if (!data.success) {
        throw new Error(data.error || "Settlement failed");
      }

      toast.success("Settlement Successful", {
        description: "Your funds have been settled successfully.",
      });

      // Refresh balance after a short delay
      setTimeout(() => {
        if (walletId) fetchBalance(walletId);
      }, 2000);
    } catch (error) {
      console.error("Settlement error:", error);
      toast.error("Error", {
        description:
          error instanceof Error ? error.message : "Settlement failed",
      });
    } finally {
      setIsLoading(false);
    }
  };

  const handleLogout = () => {
    localStorage.removeItem("wallet_id");
    localStorage.removeItem("onchain_address");
    localStorage.removeItem("offchain_address");
    router.push("/");
  };

  if (!walletId || !balance) {
    return (
      <div className="flex min-h-screen items-center justify-center bg-background">
        <div className="flex items-center space-x-2">
          <RefreshCw className="h-6 w-6 animate-spin text-orange-500" />
          <p>Loading wallet...</p>
        </div>
      </div>
    );
  }

  return (
    <div className="flex min-h-screen flex-col bg-background">
      {/* Header */}
      <header className="sticky top-0 z-10 border-b border-zinc-800 bg-zinc-950/80 backdrop-blur-sm">
        <div className="container flex h-16 items-center justify-between">
          <div className="flex items-center space-x-2">
            <Bitcoin className="h-6 w-6 text-orange-500" />
            <h1 className="text-xl font-bold">ARKane Wallet</h1>
          </div>
          <Button
            variant="ghost"
            size="icon"
            onClick={handleLogout}
            className="text-muted-foreground hover:text-white"
          >
            <LogOut className="h-5 w-5" />
            <span className="sr-only">Logout</span>
          </Button>
        </div>
      </header>

      {/* Main content */}
      <main className="container flex-1 py-6">
        <div className="grid gap-6">
          {/* Wallet ID */}
          <div className="flex items-center justify-between">
            <div className="flex items-center space-x-2">
              <Wallet className="h-5 w-5 text-muted-foreground" />
              <p className="text-sm text-muted-foreground">
                Wallet ID: {walletId.substring(0, 8)}...
                {walletId.substring(walletId.length - 8)}
              </p>
            </div>
            <Button
              variant="ghost"
              size="icon"
              className="h-8 w-8 text-muted-foreground"
              onClick={() => {
                navigator.clipboard.writeText(walletId);
                toast.success("Copied", {
                  description: "Wallet ID copied to clipboard",
                });
              }}
            >
              <Copy className="h-4 w-4" />
              <span className="sr-only">Copy wallet ID</span>
            </Button>
          </div>

          {/* Balance Card */}
          <Card className="border-zinc-800 bg-zinc-950">
            <CardHeader>
              <CardTitle className="text-xl">Wallet Balance</CardTitle>
              <CardDescription>Your current Bitcoin balance</CardDescription>
            </CardHeader>
            <CardContent>
              <div className="space-y-6">
                <div className="flex items-center justify-between">
                  <div>
                    <p className="text-sm text-muted-foreground">Total</p>
                    <h2 className="text-3xl font-bold text-orange-500">
                      {(
                        balance.offchain_balance.spendable +
                        balance.boarding_balance.spendable
                      ).toFixed(8)}{" "}
                      BTC
                    </h2>
                  </div>
                  <Button
                    variant="outline"
                    size="icon"
                    onClick={handleRefresh}
                    disabled={isLoading}
                    className="border-zinc-800 text-muted-foreground"
                  >
                    <RefreshCw
                      className={`h-4 w-4 ${isLoading ? "animate-spin" : ""}`}
                    />
                    <span className="sr-only">Refresh balance</span>
                  </Button>
                </div>

                <Separator className="bg-zinc-800" />

                <div className="space-y-4">
                  <div className="grid grid-cols-2 gap-4">
                    <div>
                      <p className="text-sm text-muted-foreground">Offchain</p>
                      <p className="text-lg font-medium">
                        {balance.offchain_balance.spendable.toFixed(8)} BTC
                      </p>
                      {balance.offchain_balance.expired > 0 && (
                        <p className="text-xs text-muted-foreground">
                          Expired: {balance.offchain_balance.expired.toFixed(8)}{" "}
                          BTC
                        </p>
                      )}
                    </div>

                    <div>
                      <p className="text-sm text-muted-foreground">Onchain</p>
                      <p className="text-lg font-medium">
                        {balance.boarding_balance.spendable.toFixed(8)} BTC
                      </p>
                      {balance.boarding_balance.pending > 0 && (
                        <p className="text-xs text-muted-foreground">
                          Pending: {balance.boarding_balance.pending.toFixed(8)}{" "}
                          BTC
                        </p>
                      )}
                      {balance.boarding_balance.expired > 0 && (
                        <p className="text-xs text-muted-foreground">
                          Expired: {balance.boarding_balance.expired.toFixed(8)}{" "}
                          BTC
                        </p>
                      )}
                    </div>
                  </div>
                </div>
                <Button
                  onClick={handleSettle}
                  disabled={isLoading}
                  variant="outline"
                  className="w-full bg-orange-400 max-w-[200px] text-black border-zinc-800 hover:bg-orange-600 hover:text-black"
                >
                  {isLoading ? (
                    <>
                      <Loader2 className="mr-2 h-4 w-4 animate-spin" />
                      Settling...
                    </>
                  ) : (
                    "Settle Funds"
                  )}
                </Button>
              </div>
            </CardContent>
          </Card>

          {/* Action Buttons */}
          <div className="grid grid-cols-3 gap-4">
            <Button
              className="flex flex-col items-center justify-center h-24 bg-zinc-900 hover:bg-zinc-800 border border-zinc-800"
              onClick={() => setIsSendModalOpen(true)}
            >
              <Send className="h-6 w-6 mb-2 text-orange-500" />
              <span>Send</span>
            </Button>
            <Button
              className="flex flex-col items-center justify-center h-24 bg-zinc-900 hover:bg-zinc-800 border border-zinc-800"
              onClick={() => setIsReceiveModalOpen(true)}
            >
              <Download className="h-6 w-6 mb-2 text-orange-500" />
              <span>Receive</span>
            </Button>
            <Button
              className="flex flex-col items-center justify-center h-24 bg-zinc-900 hover:bg-zinc-800 border border-zinc-800"
              onClick={handleFaucet}
              disabled={isLoading}
            >
              <ArrowLeftRight className="h-6 w-6 mb-2 text-orange-500" />
              <span>Faucet</span>
            </Button>
          </div>
        </div>
      </main>

      {/* Modals */}
      <SendModal
        isOpen={isSendModalOpen}
        onClose={() => setIsSendModalOpen(false)}
        walletId={walletId}
        onSuccess={handleRefresh}
      />

      <ReceiveModal
        isOpen={isReceiveModalOpen}
        onClose={() => setIsReceiveModalOpen(false)}
        onchainAddress={onchainAddress || ""}
        offchainAddress={offchainAddress || ""}
      />
    </div>
  );
}
